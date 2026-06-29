//! Virtual filesystem exposed to scripts as the `fs` module.
//!
//! A VFS is anchored at a real `root` directory. Scripts see a
//! root-relative namespace with no leading `/`: a real file
//! `root/subdir/x.txt` is addressed as `subdir/x.txt` (a leading `/` is
//! still accepted on input and treated the same as none). Paths that
//! escape the root (via `..`) are rejected.
//!
//! Reads always go straight to disk. How writes get there depends on the
//! run's [`Writes`] mode (see [`register_with`]):
//!
//! - **Buffered** — nothing touches disk until the host calls
//!   [`Vfs::flush`]. Each path has a single pending slot, so `fs.write`
//!   and `fs.remove` overwrite each other — the last call to a given path
//!   wins (write→remove→write ends up writing; remove→write→remove ends
//!   up removing). Scripts cannot read their own buffered writes, since a
//!   read reflects current disk state.
//! - **Denied** — the write functions are not registered at all.
//!
//! Either way a write creates parent directories as needed (`mkdir -p`)
//! and a removal of an absent file is a no-op.
//!
//! This is the rooted `fs` — the one `iq` and `ied` expose, where a
//! boundary is what makes a `--diff` preview mean something. `ito`'s `fs`
//! is the host filesystem instead; see [`super::rfs`].
//!
//! The module registered on the engine holds clones of the same
//! `Rc<RefCell<..>>` state the host keeps, so buffered `fs.write(...)` /
//! `fs.remove(...)` calls in the script are visible to the host's
//! `flush()` after the script returns.

use crate::scripting::finder;
use crate::scripting::pathmap::PathMapper;
use globset::{Glob, GlobBuilder, GlobMatcher};
use mlua::{Lua, Table, UserData, UserDataMethods};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

type Result<T> = mlua::Result<T>;

/// How `fs.write`/`fs.remove` reach the disk, if at all.
///
/// Chosen by the host per run (see [`register_with`]); it decides which functions
/// exist, not just what they do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Writes {
    /// No write functions at all: `fs.write`/`fs.remove` are not defined, so a
    /// script cannot queue a change, let alone make one.
    Denied,
    /// Buffered here and applied only if the host flushes, after the script
    /// returns. Because nothing lands until then, the whole change set can be
    /// previewed or abandoned — and the choice is the host's (`ied -w`), with
    /// no script-facing way to force or drop the buffer.
    Buffered,
}

impl Writes {
    /// Whether `fs.write`/`fs.remove` exist at all.
    pub fn enabled(self) -> bool {
        !matches!(self, Self::Denied)
    }

    /// Whether writes accumulate rather than landing immediately.
    ///
    /// Gates everything that reads the pending set: the host's end-of-run flush
    /// and the report flags.
    pub fn buffered(self) -> bool {
        matches!(self, Self::Buffered)
    }
}

/// A buffered action against a path. `fs.write` and `fs.remove` both
/// land in the same per-path slot, so the last call wins.
enum Action {
    /// Write this content on flush (parents auto-created).
    Write(String),
    /// Delete the path on flush (no error if it is already absent).
    Remove,
}

/// Write `content` to `path`, creating missing parents.
///
/// The single place a write actually happens, shared by [`Vfs::flush`] and the
/// host filesystem's `fs.write` ([`super::rfs`]), so the two cannot drift on
/// details like parent creation or error wording.
pub(crate) fn write_now(path: &Path, content: &str) -> std::result::Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    std::fs::write(path, content).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Delete `path`. An already-absent file is success, not an error — removing
/// something that is not there is the state the caller asked for.
pub(crate) fn remove_now(path: &Path) -> std::result::Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove {}: {e}", path.display())),
    }
}

#[derive(Default)]
struct Pending {
    /// Buffered actions: real path -> write/remove. Last call wins.
    actions: BTreeMap<PathBuf, Action>,
}

/// Handle to a virtual filesystem. Cheap to clone: clones share the
/// same root and pending-write buffer.
#[derive(Clone)]
pub struct Vfs {
    mapper: PathMapper,
    pending: Rc<RefCell<Pending>>,
}

impl Vfs {
    /// Anchor a VFS at `root`. The directory must already exist (it is
    /// canonicalized so containment checks are reliable).
    pub fn new(root: &Path) -> std::result::Result<Self, String> {
        Ok(Self {
            mapper: PathMapper::new(root).map_err(|e| e.to_string())?,
            pending: Rc::new(RefCell::new(Pending::default())),
        })
    }

    /// Translate a script-visible path into an absolute real path under
    /// the root. See [`PathMapper::to_real_abs`].
    pub(crate) fn resolve(&self, virt: &str) -> Result<PathBuf> {
        Ok(self.mapper.to_real_abs(virt)?)
    }

    /// A cloneable, `Send + Sync` path mapper over the same root.
    pub fn mapper(&self) -> PathMapper {
        self.mapper.clone()
    }

    /// Flush buffered actions (sorted by path). Writes create parent
    /// directories as needed; removes delete the file if present.
    /// Called by the host after the script returns, and only then.
    pub fn flush(&self) -> std::result::Result<(), String> {
        let pending = self.pending.borrow();
        for (path, action) in &pending.actions {
            match action {
                Action::Write(content) => write_now(path, content)?,
                Action::Remove => remove_now(path)?,
            }
        }
        Ok(())
    }

    /// List the paths whose buffered action differs from current disk
    /// state, sorted, as real filesystem paths relative to the VFS `root`
    /// (the `-C` working directory) rather than the VFS-internal `/`
    /// namespace. Uses the same "really changed" predicate as
    /// [`Vfs::diff`]: a write matching disk is not reported, and a remove
    /// of an absent file is not reported.
    pub fn changed(&self) -> std::result::Result<Vec<String>, std::io::Error> {
        let pending = self.pending.borrow();
        let mut out: Vec<String> = pending
            .actions
            .iter()
            .filter_map(|(path, action)| match action {
                Action::Write(new) => match std::fs::read_to_string(path) {
                    Ok(old) => (old != **new).then(|| Ok(self.mapper.relativize(path))),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        Some(Ok(self.mapper.relativize(path)))
                    }
                    Err(e) => Some(Err(e)),
                },
                Action::Remove => path
                    .symlink_metadata()
                    .is_ok()
                    .then(|| Ok(self.mapper.relativize(path))),
            })
            .collect::<std::result::Result<_, _>>()?;
        out.sort();
        Ok(out)
    }

    /// Render a unified diff of every pending action against current disk
    /// state. Writes whose content matches disk and removes of absent files
    /// are skipped; a write to a path with no existing file shows as an
    /// all-additions new file, and a remove of an existing file shows as an
    /// all-deletions diff. Returns an empty string when there is nothing to
    /// show.
    pub fn diff(&self) -> std::result::Result<String, std::io::Error> {
        let pending = self.pending.borrow();
        let mut out = String::new();
        for (path, action) in &pending.actions {
            let old = match std::fs::read_to_string(path) {
                Ok(s) => Some(s),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };
            let new = match action {
                Action::Write(content) => content.as_str(),
                Action::Remove => "",
            };
            if old.as_deref() == Some(new) {
                continue;
            }
            let is_new_file = old.is_none();
            let old = old.unwrap_or_default();
            let rel = self.mapper.relativize(path);
            // An empty write to a new path has no line-level diff to show, but
            // we still emit the header so callers (and humans) can see the file
            // will be created.
            if is_new_file && new.is_empty() {
                out.push_str(&format!("--- /dev/null\n+++ b/{rel}\n"));
                continue;
            }
            let diff = similar::TextDiff::from_lines(&old, new);
            out.push_str(
                &diff
                    .unified_diff()
                    .context_radius(3)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        Ok(out)
    }
}

fn build_matcher(pattern: &str) -> Result<GlobMatcher> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|g: Glob| g.compile_matcher())
        .map_err(|e| mlua::Error::runtime(format!("invalid glob pattern: {e}")))
}

/// A filesystem entry's basename is "hidden" if it starts with a dot
/// (the Unix dotfile convention), e.g. `.git`, `.terraform`.
fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

/// Walk `dir` recursively, collecting every regular file.
///
/// A symlink stands in for what it points at: a link to a file is collected as
/// a file, a link to a directory is descended into. It keeps *its own* path
/// though -- `src/link.txt` is reported under that name, not under its target's
/// -- so a link and its target are two entries, the way `ls` shows them, rather
/// than the same path twice. A link resolving outside `root` is skipped, which
/// is what keeps traversal inside the boundary the VFS enforces elsewhere.
///
/// Hidden files are skipped and hidden directories are not descended into
/// (`fs.glob` never traverses hidden entries; use `fs.finder(..):hidden()`).
fn walk(root: &Path, dir: &Path, ancestors: &mut Vec<PathBuf>, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if path.file_name().is_some_and(is_hidden) {
            continue;
        }
        // Canonicalizing resolves a link and bounds it in one step: a target
        // outside the root is dropped here. The canonical form is used for the
        // kind and the cycle check only -- what gets reported is `path`, so a
        // link keeps its own name.
        let Ok(canonical) = path.canonicalize() else {
            continue;
        };
        if !canonical.starts_with(root) {
            continue;
        }
        let kind = if ft.is_symlink() {
            let Ok(meta) = std::fs::metadata(&canonical) else {
                continue;
            };
            meta.file_type()
        } else {
            ft
        };
        if kind.is_dir() {
            // Descend only into a directory not already on this branch: a link
            // pointing at one of its own ancestors would never terminate.
            if !ancestors.contains(&canonical) {
                ancestors.push(canonical);
                walk(root, &path, ancestors, out);
                ancestors.pop();
            }
        } else if kind.is_file() {
            out.push(path);
        }
    }
}

/// One directory visited by [`walk_tree`]: the Rust side of what `fs.walk`
/// hands back per step.
struct WalkStep {
    /// The directory this step describes, as a real path.
    dir: PathBuf,
    /// Bare names of the subdirectories in it, sorted.
    dirs: Vec<String>,
    /// Bare names of the files in it, sorted.
    files: Vec<String>,
}

/// Walk `dir` top-down, one [`WalkStep`] per directory, parents before
/// children.
///
/// The shape `os.walk` established: each step names a directory and splits its
/// immediate children into subdirectories and files, as bare names rather than
/// paths, so joining them onto the step's own path is the caller's to do.
///
/// Symlink handling matches [`walk`] -- a link counts as what it points at, is
/// listed under its own name, and is dropped if it resolves outside `root`.
/// Hidden entries are included: `fs.walk` reports a directory's contents rather
/// than filtering them, and a caller that wants them gone can say so.
fn walk_tree(root: &Path, dir: &Path, ancestors: &mut Vec<PathBuf>, out: &mut Vec<WalkStep>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    let mut dirs: Vec<(String, PathBuf, PathBuf)> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let Ok(canonical) = path.canonicalize() else {
            continue;
        };
        if !canonical.starts_with(root) {
            continue;
        }
        let kind = if ft.is_symlink() {
            let Ok(meta) = std::fs::metadata(&canonical) else {
                continue;
            };
            meta.file_type()
        } else {
            ft
        };
        if kind.is_dir() {
            dirs.push((name, path, canonical));
        } else if kind.is_file() {
            files.push(name);
        }
    }

    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    files.sort();

    out.push(WalkStep {
        dir: dir.to_path_buf(),
        dirs: dirs.iter().map(|(n, _, _)| n.clone()).collect(),
        files,
    });

    // Top-down: this directory is reported before any of its children, so a
    // caller sees a parent before the steps beneath it.
    for (_, path, canonical) in dirs {
        if ancestors.contains(&canonical) {
            continue;
        }
        ancestors.push(canonical);
        walk_tree(root, &path, ancestors, out);
        ancestors.pop();
    }
}

/// The rooted VFS as a finder [`Source`]: the walk starts at the root and every
/// reported path is root-relative.
///
/// This is where the boundary lives. [`render`](finder::Source::render) returns
/// `None` for anything that resolves outside the root, which is how a symlink
/// pointing out is dropped from the results — the finder itself never learns
/// what a root is, so the rooted and host searches share one implementation
/// without sharing a policy flag.
pub struct RootedSource {
    mapper: PathMapper,
}

impl finder::Source for RootedSource {
    fn start(&self) -> PathBuf {
        self.mapper.root().to_path_buf()
    }

    fn render(&self, path: &Path) -> Option<String> {
        // `contained` canonicalizes, so a link whose target leaves the root is
        // refused here even though its own path looks inside.
        if !self.mapper.contained(path) {
            return None;
        }
        Some(self.mapper.to_virtual(path))
    }

    /// The rooted traversals do not distinguish links: a link is simply the file
    /// or directory it names, so a link to an in-root directory is walked like
    /// the directory it stands for. The boundary is unaffected -- `render` tests
    /// the canonical location, so following a link can only reach files already
    /// under the root, and one pointing out is dropped exactly as before.
    /// (`fs.is_symlink` can still identify one; it is the predicates, not the
    /// walk, that give a script a way to ask.)
    fn always_follow(&self) -> bool {
        true
    }
}

/// The rooted `fs.finder`'s handle.
///
/// A newtype rather than an alias: `UserData` is implemented here and the
/// generic finder lives in another module, so the orphan rule needs a local
/// type. Cloning shares the builder state, as the inner finder does.
#[derive(Clone)]
pub struct Finder(finder::Finder<RootedSource>);

impl UserData for Finder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        finder::add_methods(methods, |f: &Finder| &f.0);
    }
}

/// Register the `fs` module on `lua`. The module's functions close over a
/// clone of the VFS's shared state, so writes/removes they record are
/// visible through the host's `Vfs` handle on flush.
///
pub fn register(lua: &Lua, vfs: &Vfs) -> Result<()> {
    register_with(lua, vfs, Writes::Buffered)
}

/// Register `fs` with the given write mode.
///
/// - [`Writes::Denied`] installs no write functions at all, so `fs.write` is
///   absent rather than present-and-refusing — simpler to audit, and
///   feature-testable from a script via `if fs.write then`.
/// - [`Writes::Buffered`] is the VFS proper: writes queue and the host applies
///   them after the script returns, which is what makes `--diff` and
///   `--list-changed` meaningful.
///
/// When the buffer reaches disk is the host's decision alone (`ied -w`), never
/// the script's: a script that could flush or discard mid-run would make
/// "nothing was written" untrue while a preview flag was in effect.
pub fn register_with(lua: &Lua, vfs: &Vfs, writes: Writes) -> Result<()> {
    let module = lua.create_table()?;

    // fs.list(dir) -> {String}
    // Entries (files and directories) directly under `dir`, as
    // script-visible root-relative paths. Sorted.
    let v = vfs.clone();
    module.set(
        "list",
        lua.create_function(move |_, dir: String| -> Result<Vec<String>> {
            let real = v.resolve(&dir)?;
            let entries = std::fs::read_dir(&real)
                .map_err(|e| mlua::Error::runtime(format!("list {}: {e}", real.display())))?;
            let mut out: Vec<String> = Vec::new();
            for entry in entries.flatten() {
                out.push(v.mapper.to_virtual(&entry.path()));
            }
            out.sort();
            Ok(out)
        })?,
    )?;

    // fs.walk([dir]) -> {{path=String, dirs={String}, files={String}}}
    // Top-down, one entry per directory visited, as `os.walk` yields
    // `(dirpath, dirnames, filenames)`. `dir` defaults to the root.
    let v = vfs.clone();
    module.set(
        "walk",
        lua.create_function(move |lua, dir: Option<String>| -> Result<Vec<Table>> {
            let start = v.resolve(dir.as_deref().unwrap_or(""))?;
            let root = v.mapper.root();
            let mut steps: Vec<WalkStep> = Vec::new();
            let mut ancestors: Vec<PathBuf> =
                vec![start.canonicalize().unwrap_or_else(|_| start.clone())];
            walk_tree(root, &start, &mut ancestors, &mut steps);

            let mut out = Vec::with_capacity(steps.len());
            for step in steps {
                let t = lua.create_table()?;
                t.set("path", v.mapper.to_virtual(&step.dir))?;
                t.set("dirs", step.dirs)?;
                t.set("files", step.files)?;
                out.push(t);
            }
            Ok(out)
        })?,
    )?;

    // fs.glob(pattern) -> {String}
    // `pattern` is a script-visible glob rooted at the VFS root (e.g.
    // "**/*.txt"). Matches regular files under the root. Sorted. Hidden
    // files and dot-directories are always excluded; use
    // `fs.finder(..):hidden()` to include them.
    let v = vfs.clone();
    module.set(
        "glob",
        lua.create_function(move |_, pattern: String| -> Result<Vec<String>> {
            let real_pat = v.resolve(&pattern)?;
            let matcher = build_matcher(&real_pat.to_string_lossy())?;
            let mut files: Vec<PathBuf> = Vec::new();
            let root = v.mapper.root();
            let mut ancestors: Vec<PathBuf> = vec![root.to_path_buf()];
            walk(root, root, &mut ancestors, &mut files);
            files.sort();
            let mut out: Vec<String> = files
                .iter()
                .filter(|p| matcher.is_match(p))
                .map(|p| v.mapper.to_virtual(p))
                .collect();
            out.sort();
            Ok(out)
        })?,
    )?;

    // fs.finder() -> Finder
    // Start an `fd`-like search over the VFS root. There is no pattern
    // argument -- an unfiltered finder means "everything under the root", and
    // each condition is added as its own named filter. There is no start
    // directory either: the root is the only place a rooted search can begin,
    // so the host form's `dir` argument would have nothing to name here.
    // Chain `:name()`/`:path()`/`:ext()`, `:exclude()`, `:files()`/`:dirs()`/
    // `:type()`, `:depth()`/`:min_depth()`/`:max_depth()`, `:hidden()`/
    // `:no_ignore()`/`:unrestricted()`, `:sort()`/`:limit()`, then a terminal:
    // `:find()`, `:first()`, `:count()` or `:any()`.
    let v = vfs.clone();
    module.set(
        "finder",
        lua.create_function(move |_, ()| -> Result<Finder> {
            Ok(Finder(finder::Finder::new(RootedSource {
                mapper: v.mapper.clone(),
            })))
        })?,
    )?;

    // fs.read(path) -> String   (reads current disk state)
    let v = vfs.clone();
    module.set(
        "read",
        lua.create_function(move |_, path: String| -> Result<String> {
            let real = v.resolve(&path)?;
            std::fs::read_to_string(&real)
                .map_err(|e| mlua::Error::runtime(format!("read {}: {e}", real.display())))
        })?,
    )?;

    if writes.enabled() {
        // fs.write(path, content)
        // Queued until flush; last call wins, shares a slot with fs.remove;
        // parents auto-created on flush.
        let v = vfs.clone();
        module.set(
            "write",
            lua.create_function(move |_, (path, content): (String, String)| -> Result<()> {
                let real = v.resolve(&path)?;
                v.pending
                    .borrow_mut()
                    .actions
                    .insert(real, Action::Write(content));
                Ok(())
            })?,
        )?;

        // fs.remove(path)
        // Queued until flush; last call wins, shares a slot with fs.write; an
        // absent file is a no-op on flush.
        let v = vfs.clone();
        module.set(
            "remove",
            lua.create_function(move |_, path: String| -> Result<()> {
                let real = v.resolve(&path)?;
                v.pending.borrow_mut().actions.insert(real, Action::Remove);
                Ok(())
            })?,
        )?;
    }

    // fs.exists(path) -> bool   (current disk state; in-root only)
    // Out-of-root paths resolve-error rather than silently report false.
    // Follows links, so a dangling link reports false.
    let v = vfs.clone();
    module.set(
        "exists",
        lua.create_function(move |_, path: String| -> Result<bool> {
            let real = v.resolve(&path)?;
            Ok(real.metadata().is_ok())
        })?,
    )?;

    // fs.is_file(path) -> bool  (follows links, as the traversals do: a link is
    // simply the file it names. `resolve` has already refused anything whose
    // canonical location leaves the root, so following cannot escape.)
    let v = vfs.clone();
    module.set(
        "is_file",
        lua.create_function(move |_, path: String| -> Result<bool> {
            let real = v.resolve(&path)?;
            Ok(real.metadata().map(|m| m.is_file()).unwrap_or(false))
        })?,
    )?;

    // fs.is_dir(path) -> bool   (follows links)
    let v = vfs.clone();
    module.set(
        "is_dir",
        lua.create_function(move |_, path: String| -> Result<bool> {
            let real = v.resolve(&path)?;
            Ok(real.metadata().map(|m| m.is_dir()).unwrap_or(false))
        })?,
    )?;

    // fs.is_symlink(path) -> bool  (the only predicate that does *not* follow:
    // it asks about the entry itself, so it is true for a dangling link -- the
    // one case where a path reports `is_symlink` but not `exists`. A link
    // pointing *out* of the root raises like any other escape, since `resolve`
    // tests the canonical location before this ever runs.)
    let v = vfs.clone();
    module.set(
        "is_symlink",
        lua.create_function(move |_, path: String| -> Result<bool> {
            let real = v.resolve(&path)?;
            Ok(real
                .symlink_metadata()
                .map(|m| m.is_symlink())
                .unwrap_or(false))
        })?,
    )?;

    lua.globals().set("fs", module)?;
    Ok(())
}
