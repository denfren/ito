//! Virtual filesystem exposed to scripts as the `fs` module.
//!
//! A VFS is anchored at a real `root` directory. Scripts see an
//! absolute namespace rooted at `/`: a real file `root/subdir/x.txt`
//! is addressed as `/subdir/x.txt`. Paths that escape the root (via
//! `..`) are rejected.
//!
//! Reads go straight to disk. Writes and removals are *buffered*:
//! nothing touches disk until the host calls [`Vfs::flush`]. Each path
//! has a single pending slot, so `fs::write` and `fs::remove` overwrite
//! each other — the last call to a given path wins (write→remove→write
//! ends up writing; remove→write→remove ends up removing). On flush,
//! writes create parent directories as needed (`mkdir -p`), and removals
//! delete the file if it exists. Scripts cannot read their own buffered
//! writes — a read always reflects current disk state.
//!
//! The module registered on the engine holds clones of the same
//! `Rc<RefCell<..>>` state the host keeps, so `fs::write(...)` /
//! `fs::remove(...)` calls in the script are visible to the host's
//! `flush()` after the script returns.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use globset::{Glob, GlobBuilder, GlobMatcher};
use rhai::{Engine, EvalAltResult, ImmutableString, Module};

use crate::scripting::pathmap::PathMapper;

type Result<T> = std::result::Result<T, Box<EvalAltResult>>;

/// A buffered action against a path. `fs::write` and `fs::remove` both
/// land in the same per-path slot, so the last call wins.
enum Action {
    /// Write this content on flush (parents auto-created).
    Write(String),
    /// Delete the path on flush (no error if it is already absent).
    Remove,
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
    /// Called by the host after the script returns, and by the
    /// script-facing `fs::flush()` (which then clears the buffer).
    pub fn flush(&self) -> std::result::Result<(), String> {
        let pending = self.pending.borrow();
        for (path, action) in &pending.actions {
            match action {
                Action::Write(content) => {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
                    }
                    std::fs::write(path, content)
                        .map_err(|e| format!("write {}: {e}", path.display()))?;
                }
                Action::Remove => match std::fs::remove_file(path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(format!("remove {}: {e}", path.display())),
                },
            }
        }
        Ok(())
    }

    /// Drop every buffered action without applying it. Used by the
    /// script-facing `fs::flush()` after it has written to disk, so the
    /// now-applied actions are not flushed (or reported) a second time.
    pub fn clear(&self) {
        self.pending.borrow_mut().actions.clear();
    }

    /// List the paths whose buffered action differs from current disk
    /// state, sorted, as real filesystem paths relative to the VFS `root`
    /// (the `-C` working directory) rather than the VFS-internal `/`
    /// namespace. Uses the same "really changed" predicate as
    /// [`Vfs::diff`]: a write matching disk is not reported, and a remove
    /// of an absent file is not reported.
    pub fn changed(&self) -> Vec<String> {
        let pending = self.pending.borrow();
        let mut out: Vec<String> = pending
            .actions
            .iter()
            .filter(|(path, action)| match action {
                Action::Write(new) => std::fs::read_to_string(path).unwrap_or_default() != **new,
                Action::Remove => path.symlink_metadata().is_ok(),
            })
            .map(|(path, _)| self.mapper.relativize(path))
            .collect();
        out.sort();
        out
    }

    /// Render a unified diff of every pending action against current disk
    /// state. Writes whose content matches disk and removes of absent files
    /// are skipped; a write to a path with no existing file shows as an
    /// all-additions new file, and a remove of an existing file shows as an
    /// all-deletions diff. Returns an empty string when there is nothing to
    /// show.
    pub fn diff(&self) -> String {
        let pending = self.pending.borrow();
        let mut out = String::new();
        for (path, action) in &pending.actions {
            let old = std::fs::read_to_string(path).unwrap_or_default();
            let new = match action {
                Action::Write(content) => content.as_str(),
                Action::Remove => "",
            };
            if old == new {
                continue;
            }
            let rel = self.mapper.relativize(path);
            let diff = similar::TextDiff::from_lines(&old, new);
            out.push_str(
                &diff
                    .unified_diff()
                    .context_radius(3)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        out
    }
}

fn build_matcher(pattern: &str) -> Result<GlobMatcher> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|g: Glob| g.compile_matcher())
        .map_err(|e| format!("invalid glob pattern: {e}").into())
}

/// A compiled glob filter that knows what it matches against. The mode
/// is auto-detected from the pattern (see [`GlobFilter::new`]).
#[derive(Clone)]
struct GlobFilter {
    matcher: GlobMatcher,
    /// `true` if matched against the full script-visible path, `false`
    /// if matched against the basename only.
    anchored: bool,
}

impl GlobFilter {
    /// Compile `pattern`, auto-detecting the match mode:
    ///
    /// - leading `/`        → matched against the full script-visible
    ///   path (anchored, e.g. `/src/**/*.rs`).
    /// - contains `/` or `**` → also matched against the full path (a
    ///   multi-segment pattern only makes sense against a path).
    /// - otherwise          → matched against the basename, like `fd`
    ///   (e.g. `*.rs` matches any file named `*.rs` at any depth).
    fn new(pattern: &str) -> Result<Self> {
        if pattern.is_empty() {
            return Err("empty glob pattern".into());
        }
        let anchored = pattern.starts_with('/') || pattern.contains('/') || pattern.contains("**");
        let matcher = build_matcher(pattern)?;
        Ok(Self { matcher, anchored })
    }

    /// Test the filter against an entry's full virtual path and basename.
    fn is_match(&self, virt: &str, name: &str) -> bool {
        if self.anchored {
            self.matcher.is_match(virt)
        } else {
            self.matcher.is_match(name)
        }
    }
}

/// A filesystem entry's basename is "hidden" if it starts with a dot
/// (the Unix dotfile convention), e.g. `.git`, `.terraform`.
fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

/// Walk `dir` recursively, collecting every regular file. Symlinks are
/// skipped so containment under `root` survives. Hidden files are
/// skipped and hidden directories are not descended into (`fs::glob`
/// never traverses hidden entries; use `fs::finder(..).hidden()` for
/// that).
fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if path.file_name().is_some_and(is_hidden) {
            continue;
        }
        if ft.is_dir() {
            walk(root, &path, out);
        } else if ft.is_file()
            && let Ok(canonical) = path.canonicalize()
            && canonical.starts_with(root)
        {
            out.push(canonical);
        }
    }
}

/// A single entry discovered by [`walk_entries`].
struct Entry {
    /// Canonical real path under the root.
    path: PathBuf,
    /// `true` if the entry is a directory (vs. a regular file).
    is_dir: bool,
    /// Depth below the walk root: direct children are depth 1.
    depth: usize,
}

/// Walk `dir` recursively, recording both files and directories along
/// with their depth (direct children of the walk root are depth 1).
/// Symlinks are skipped so containment under `root` survives. When
/// `include_hidden` is false, hidden entries are skipped and hidden
/// directories are not descended into.
fn walk_entries(root: &Path, dir: &Path, depth: usize, include_hidden: bool, out: &mut Vec<Entry>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if !include_hidden && path.file_name().is_some_and(is_hidden) {
            continue;
        }
        let Ok(canonical) = path.canonicalize() else {
            continue;
        };
        if !canonical.starts_with(root) {
            continue;
        }
        if ft.is_dir() {
            out.push(Entry {
                path: canonical.clone(),
                is_dir: true,
                depth,
            });
            walk_entries(root, &path, depth + 1, include_hidden, out);
        } else if ft.is_file() {
            out.push(Entry {
                path: canonical,
                is_dir: false,
                depth,
            });
        }
    }
}

/// Restrict matches to files only, directories only, or either.
#[derive(Clone, Copy, Default, PartialEq)]
enum TypeFilter {
    #[default]
    Any,
    File,
    Dir,
}

/// Inclusive depth bounds, measured from the VFS root (direct children
/// are depth 1). `None` on either side means unbounded.
#[derive(Clone, Copy, Default)]
struct DepthBound {
    min: Option<usize>,
    max: Option<usize>,
}

impl DepthBound {
    fn contains(&self, depth: usize) -> bool {
        self.min.is_none_or(|m| depth >= m) && self.max.is_none_or(|m| depth <= m)
    }
}

/// Accumulated state of a [`Finder`]. All filters AND together; an entry
/// is yielded only if it satisfies the type, depth, every positive glob,
/// and none of the negated globs.
#[derive(Default)]
struct FinderState {
    type_filter: TypeFilter,
    depth: DepthBound,
    /// Include hidden entries (dotfiles and dot-directories). Off by
    /// default, matching `fd`; flipped on by `.hidden()`.
    include_hidden: bool,
    /// Positive glob filters (the seed plus any `.glob()`/`.name()`).
    globs: Vec<GlobFilter>,
    /// Negated glob filters (`.not_glob()`/`.not_name()`).
    not_globs: Vec<GlobFilter>,
}

/// A chainable file finder, à la `fd`. Built by `fs::finder(glob)`,
/// narrowed by chained filters, and run with `.find()`. Cheap to clone:
/// clones share the same builder state (mirrors the `hcl` builders).
#[derive(Clone)]
pub struct Finder {
    vfs: Vfs,
    state: Rc<RefCell<FinderState>>,
}

impl Finder {
    /// Run the walk and return matching script-visible paths, sorted.
    fn find(&self) -> Result<rhai::Array> {
        let state = self.state.borrow();
        let root = self.vfs.mapper.root();
        let mut entries: Vec<Entry> = Vec::new();
        walk_entries(root, root, 1, state.include_hidden, &mut entries);

        let mut out: Vec<String> = Vec::new();
        for entry in &entries {
            match state.type_filter {
                TypeFilter::File if entry.is_dir => continue,
                TypeFilter::Dir if !entry.is_dir => continue,
                _ => {}
            }
            if !state.depth.contains(entry.depth) {
                continue;
            }
            let virt = self.vfs.mapper.to_virtual(&entry.path);
            let name = entry
                .path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !state.globs.iter().all(|g| g.is_match(&virt, &name)) {
                continue;
            }
            if state.not_globs.iter().any(|g| g.is_match(&virt, &name)) {
                continue;
            }
            out.push(virt);
        }
        out.sort();
        Ok(out.into_iter().map(rhai::Dynamic::from).collect())
    }
}

/// Apply an integer (exact depth) or a range to a [`DepthBound`].
fn depth_from_int(n: rhai::INT) -> Result<DepthBound> {
    let d = usize::try_from(n).map_err(|_| -> Box<EvalAltResult> {
        format!("depth must be non-negative, got {n}").into()
    })?;
    Ok(DepthBound {
        min: Some(d),
        max: Some(d),
    })
}

/// Register the chainable `Finder` methods on the engine.
fn register_finder(engine: &mut Engine) {
    engine.register_type_with_name::<Finder>("Finder");

    fn push_glob(f: &Finder, pattern: &str, negate: bool) -> Result<Finder> {
        let filter = GlobFilter::new(pattern)?;
        let mut state = f.state.borrow_mut();
        if negate {
            state.not_globs.push(filter);
        } else {
            state.globs.push(filter);
        }
        drop(state);
        Ok(f.clone())
    }

    engine.register_fn("files", |f: &mut Finder| {
        f.state.borrow_mut().type_filter = TypeFilter::File;
        f.clone()
    });
    engine.register_fn("dirs", |f: &mut Finder| {
        f.state.borrow_mut().type_filter = TypeFilter::Dir;
        f.clone()
    });

    // .hidden(): include hidden entries (dotfiles and dot-directories),
    // which are excluded by default.
    engine.register_fn("hidden", |f: &mut Finder| {
        f.state.borrow_mut().include_hidden = true;
        f.clone()
    });

    // .glob(pattern) / .name(pattern): name is an alias documented as
    // "matches the file or directory name", but both go through the same
    // auto-detecting glob compiler.
    engine.register_fn("glob", |f: &mut Finder, pattern: ImmutableString| {
        push_glob(f, &pattern, false)
    });
    engine.register_fn("name", |f: &mut Finder, pattern: ImmutableString| {
        push_glob(f, &pattern, false)
    });
    engine.register_fn("not_glob", |f: &mut Finder, pattern: ImmutableString| {
        push_glob(f, &pattern, true)
    });
    engine.register_fn("not_name", |f: &mut Finder, pattern: ImmutableString| {
        push_glob(f, &pattern, true)
    });

    // .ext(e) / .not_ext(e): match a bare extension (no dot) against the
    // basename, compiled to a `*.{e}` basename glob.
    engine.register_fn("ext", |f: &mut Finder, ext: ImmutableString| {
        push_glob(f, &format!("*.{ext}"), false)
    });
    engine.register_fn("not_ext", |f: &mut Finder, ext: ImmutableString| {
        push_glob(f, &format!("*.{ext}"), true)
    });

    // .depth(n): exact depth. .depth(a..b) / .depth(a..=b): a range.
    engine.register_fn("depth", |f: &mut Finder, n: rhai::INT| -> Result<Finder> {
        f.state.borrow_mut().depth = depth_from_int(n)?;
        Ok(f.clone())
    });
    engine.register_fn(
        "depth",
        |f: &mut Finder, r: std::ops::Range<rhai::INT>| -> Result<Finder> {
            let min = usize::try_from(r.start).ok();
            let max = r.end.checked_sub(1).and_then(|e| usize::try_from(e).ok());
            f.state.borrow_mut().depth = DepthBound { min, max };
            Ok(f.clone())
        },
    );
    engine.register_fn(
        "depth",
        |f: &mut Finder, r: std::ops::RangeInclusive<rhai::INT>| -> Result<Finder> {
            let min = usize::try_from(*r.start()).ok();
            let max = usize::try_from(*r.end()).ok();
            f.state.borrow_mut().depth = DepthBound { min, max };
            Ok(f.clone())
        },
    );

    engine.register_fn("find", |f: &mut Finder| f.find());
}

/// Build the `fs` module backed by `vfs`. The module's functions close
/// over a clone of the VFS's shared state, so writes/removes they record
/// are visible through the host's `Vfs` handle on flush.
///
/// `allow_flush` gates the script-facing `fs::flush()` (`--unsafe-fs-flush`);
/// `dry_run` is the `-n` state, under which `fs::flush()` errors rather than
/// touch disk if it would actually change files.
pub fn register(engine: &mut Engine, vfs: &Vfs, allow_flush: bool, dry_run: bool) {
    register_finder(engine);

    let mut module = Module::new();

    // fs::list(dir) -> [String]
    // Entries (files and directories) directly under `dir`, as
    // script-visible absolute paths. Sorted.
    let v = vfs.clone();
    module.set_native_fn("list", move |dir: ImmutableString| -> Result<rhai::Array> {
        let real = v.resolve(&dir)?;
        let entries = std::fs::read_dir(&real).map_err(|e| -> Box<EvalAltResult> {
            format!("list {}: {e}", real.display()).into()
        })?;
        let mut out: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            out.push(v.mapper.to_virtual(&entry.path()));
        }
        out.sort();
        Ok(out.into_iter().map(rhai::Dynamic::from).collect())
    });

    // fs::glob(pattern) -> [String]
    // `pattern` is a script-visible absolute glob (e.g. "/**/*.txt").
    // Matches regular files under the root. Sorted. Hidden files and
    // dot-directories are always excluded; use `fs::finder(..).hidden()`
    // to include them.
    let v = vfs.clone();
    module.set_native_fn(
        "glob",
        move |pattern: ImmutableString| -> Result<rhai::Array> {
            // Resolve the pattern against the root, then match real paths.
            let real_pat = v.resolve(&pattern)?;
            let matcher = build_matcher(&real_pat.to_string_lossy())?;
            let mut files: Vec<PathBuf> = Vec::new();
            let root = v.mapper.root();
            walk(root, root, &mut files);
            files.sort();
            let mut out: Vec<String> = files
                .iter()
                .filter(|p| matcher.is_match(p))
                .map(|p| v.mapper.to_virtual(p))
                .collect();
            out.sort();
            Ok(out.into_iter().map(rhai::Dynamic::from).collect())
        },
    );

    // fs::finder(glob) -> Finder
    // Start an `fd`-like search. `glob` seeds the candidate set, scanned
    // from the VFS root: a leading `/` (or any `/`/`**`) matches the full
    // path, otherwise the bare name at any depth. Chain `.files()`,
    // `.dirs()`, `.ext()`, `.glob()`/`.name()`, `.not_*()`, `.depth()`,
    // `.hidden()` to narrow (all AND together), then `.find()` for a
    // sorted array. Hidden entries are excluded unless `.hidden()` is set.
    let v = vfs.clone();
    module.set_native_fn("finder", move |glob: ImmutableString| -> Result<Finder> {
        let seed = GlobFilter::new(&glob)?;
        let state = FinderState {
            globs: vec![seed],
            ..Default::default()
        };
        Ok(Finder {
            vfs: v.clone(),
            state: Rc::new(RefCell::new(state)),
        })
    });

    // fs::read(path) -> String   (reads current disk state)
    let v = vfs.clone();
    module.set_native_fn("read", move |path: ImmutableString| -> Result<String> {
        let real = v.resolve(&path)?;
        std::fs::read_to_string(&real).map_err(|e| format!("read {}: {e}", real.display()).into())
    });

    // fs::write(path, content)   (buffered until flush; last call wins,
    // shares a slot with fs::remove; parents auto-created on flush)
    let v = vfs.clone();
    module.set_native_fn(
        "write",
        move |path: ImmutableString, content: ImmutableString| -> Result<()> {
            let real = v.resolve(&path)?;
            v.pending
                .borrow_mut()
                .actions
                .insert(real, Action::Write(content.to_string()));
            Ok(())
        },
    );

    // fs::remove(path)           (buffered deletion until flush; last call
    // wins, shares a slot with fs::write; absent file is a no-op on flush)
    let v = vfs.clone();
    module.set_native_fn("remove", move |path: ImmutableString| -> Result<()> {
        let real = v.resolve(&path)?;
        v.pending.borrow_mut().actions.insert(real, Action::Remove);
        Ok(())
    });

    // fs::exists(path) -> bool   (current disk state; in-root only)
    // Out-of-root paths resolve-error rather than silently report false.
    let v = vfs.clone();
    module.set_native_fn("exists", move |path: ImmutableString| -> Result<bool> {
        let real = v.resolve(&path)?;
        Ok(real.symlink_metadata().is_ok())
    });

    // fs::is_file(path) -> bool  (regular file; symlinks report false to
    // match what list/glob would actually traverse)
    let v = vfs.clone();
    module.set_native_fn("is_file", move |path: ImmutableString| -> Result<bool> {
        let real = v.resolve(&path)?;
        Ok(real
            .symlink_metadata()
            .map(|m| m.is_file())
            .unwrap_or(false))
    });

    // fs::is_dir(path) -> bool   (directory; symlinks report false)
    let v = vfs.clone();
    module.set_native_fn("is_dir", move |path: ImmutableString| -> Result<bool> {
        let real = v.resolve(&path)?;
        Ok(real.symlink_metadata().map(|m| m.is_dir()).unwrap_or(false))
    });

    // fs::flush()              (mid-script flush; gated behind
    // --unsafe-fs-flush). Applies all buffered writes/removes to disk
    // immediately, then clears the buffer so they are neither re-flushed
    // by the host nor shown by --diff/--list-changed. Under --dry-run it
    // errors instead of writing if the flush would change any file.
    let v = vfs.clone();
    module.set_native_fn("flush", move || -> Result<()> {
        if !allow_flush {
            return Err("fs::flush() is disabled; pass --unsafe-fs-flush to enable it".into());
        }
        if dry_run && !v.changed().is_empty() {
            return Err("fs::flush() would change files under --dry-run; refusing to write".into());
        }
        v.flush().map_err(|e| -> Box<EvalAltResult> { e.into() })?;
        v.clear();
        Ok(())
    });

    // fs::clear()              (drop all buffered writes/removes without
    // applying them; ungated). After this the buffer is empty, so nothing
    // is flushed by the host or shown by --diff/--list-changed.
    let v = vfs.clone();
    module.set_native_fn("clear", move || -> Result<()> {
        v.clear();
        Ok(())
    });

    engine.register_static_module("fs", module.into());
}
