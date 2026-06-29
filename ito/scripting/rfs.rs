//! The host filesystem exposed to scripts as the `fs` module.
//!
//! This is `ito`'s `fs`: the real filesystem, unrooted. A relative path
//! resolves against the **work directory** — the `-C`-resolved cwd, which the
//! runner has already `chdir`ed into, so `std::fs` does the resolving for free
//! and there is no mapper in the middle. An absolute path means what it says,
//! and `..` climbs out of the work directory rather than erroring.
//!
//! That is the whole difference from the rooted VFS in [`super::vfs`]: for any
//! path the VFS would have accepted, this resolves to the same file. Escapes
//! are the only behavioural delta — which is why the two live side by side
//! rather than sharing a flag. `iq`/`ied` keep the rooted namespace, where a
//! `--diff` preview can promise a boundary; `ito` already spawns processes and
//! opens sockets, so a filesystem boundary would be a pretence.
//!
//! `fs.cwd()` reports that work directory, absolute. It exists only here: the
//! rooted VFS resolves against a root the script is not meant to locate on the
//! host, so leaking the path there would hand an `iq`/`ied` script something its
//! own `fs` cannot act on.
//!
//! `fs.canonicalize(path)` is its counterpart and host-only for the same
//! reason: it puts any path a script is holding into the same absolute,
//! symlink-resolved form, so the two are directly comparable — the "is this
//! under the work directory, and where?" test, once `path.relative_to` has
//! turned the answer back into a relative path. It resolves links where the
//! traversals deliberately do not (they report a link under its own name), and
//! it errors on a path that does not exist, resolving a link being a read.
//!
//! `fs.write_new` is the atomic create-if-absent write, and so the lock
//! primitive: one `O_CREAT|O_EXCL` open, `true` if this call created the file
//! and `false` if it was already there, with `fs.remove` as the release. Losing
//! the race is a return value rather than an error because it is the question
//! being asked; an existing *non-file* raises instead, that being a name no
//! `fs.remove` can free. It is host-only for the reason the mutators are, and
//! more sharply: "created now, atomically" is precisely what a buffered write
//! cannot express, the answer being needed before the script continues. There
//! is deliberately no lock *handle* — release-on-GC would promise a lifetime
//! Luau does not guarantee, and waiting, staleness and ownership all need
//! something this module cannot know (a pid, a host, a deadline), which is what
//! the content argument is for.
//!
//! Writes land immediately (there is nothing to preview and nothing to flush),
//! and the operations that have no previewable form — `fs.mkdir`, `fs.rmdir`,
//! `fs.rename`, `fs.copy`, `fs.symlink` — live here rather than in the VFS. They
//! earn their place from `proc`: a child process needs a directory to be there
//! *before* it runs, and shelling out to `mkdir` to get one would be a poor
//! answer.
//!
//! **The predicates follow links; only `fs.is_symlink` does not.** `fs.is_file`/
//! `fs.is_dir`/`fs.exists` resolve the final component and answer about the
//! *target*, so a link to a file is a file and a dangling link does not exist.
//! That matches the traversals (a link stands for what it names) and the rooted
//! VFS, so the predicates mean one thing under every binary — the rooted side
//! can follow safely because its `resolve` tests the canonical location first,
//! so an outward link raises instead of reporting. `fs.is_symlink` asks about
//! the entry itself, which makes both questions expressible:
//! `fs.is_dir(p) and not fs.is_symlink(p)` is "a real directory". It is a
//! predicate rather than a capability, so unlike the rest of this paragraph's
//! neighbours it exists in the rooted `fs` too.
//!
//! `fs.readlink`/`fs.symlink` are the link API, host-only for the same reason
//! the mutators above are. `readlink` is one hop and unresolved — the target
//! verbatim, relative if written that way, readable even when dangling — which
//! is precisely the question `fs.canonicalize` does *not* answer.
//!
//! **A traversal answers in the terms it was asked.** `fs.list`/`fs.walk`/
//! `fs.glob`/`fs.finder` prefix their results with the start directory exactly
//! as the script spelled it: the default `"."` gives `sub/b.txt`, a `".."` start
//! gives `../b/y`, and an absolute start gives absolute paths. This is
//! `walkdir`'s rule, and so `find`'s and `fd`'s — `find ..` prints `../b/y` too.
//!
//! One spelling per traversal is the point. Deciding it per entry (relative
//! under the work directory, absolute outside) returned both shapes from a
//! single call, and since a seed glob is matched against the reported path, no
//! one anchored pattern could match the whole result set — silently, with a
//! plausible non-zero count. Taking the prefix from the call site instead makes
//! pattern and result agree by construction.
//!
//! For every path the rooted VFS could have named, this still matches it: a
//! default start directory reports the same relative spelling.
//!
//! Symlinks are followed rather than bounded: a link is reported under its own
//! name and counted as what it points at, wherever that lives. A link pointing
//! back up its own ancestry is still not descended into twice, so a cycle
//! terminates.

use crate::scripting::finder;
use crate::scripting::vfs::{remove_now, write_now};
use globset::{Glob, GlobBuilder, GlobMatcher};
use mlua::{Lua, Table, UserData, UserDataMethods};
use std::path::{Path, PathBuf};

type Result<T> = mlua::Result<T>;

/// Absolute form of a script-given path, without touching the disk.
///
/// A relative path is joined onto the process cwd — which `-C` has already set
/// to the work directory — so this agrees with what `std::fs` would do on its
/// own. Traversals work in this form internally: a glob matches against the full
/// path, and the cycle checks compare real locations. [`out_path`] shortens it
/// again on the way back to the script.
///
/// The result is lexically normalized (`.` dropped, `..` popped) rather than
/// canonicalized: symlinks are left alone, because a link's *own* path is what
/// the traversals report. Normalizing matters for more than looks — the default
/// start directory is `"."`, and joining that verbatim would prefix every
/// reported path with `/./` and put it out of reach of an anchored glob.
fn absolutize(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            Err(_) => path.to_path_buf(),
        }
    };
    let mut out = PathBuf::new();
    for comp in joined.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// How a traversal spells the paths it reports.
///
/// The rule is `walkdir`'s, and through it `find`'s and `fd`'s: **the start
/// directory you asked with is the prefix you get back**, joined with each
/// entry's descent below it. `fs.walk("..")` reports `../b/y`, an absolute start
/// directory reports absolute paths, and the default `"."` reports `sub/b.txt` —
/// the same output `find .` and `fd` give from the same place.
///
/// What this replaces is a per-entry test ("is it under the work directory?"),
/// which decided each path's spelling on its own and so returned both spellings
/// from one traversal — leaving no single anchored glob able to match the whole
/// result set, silently. Deriving the prefix from the call instead makes the
/// spelling uniform per traversal and predictable from the call site, and since
/// a seed glob is matched against the reported path, pattern and result now
/// agree by construction.
///
/// The absolute form stays the internal representation regardless: globs match
/// against the reported path but the walk itself, and the symlink-cycle checks,
/// work in real absolute locations. Only the rendering on the way back is short.
struct OutStyle {
    /// The start directory exactly as the script spelled it, minus a trailing
    /// slash, or `None` when it was already absolute (nothing to re-prefix).
    prefix: Option<String>,
    /// Absolute form of that start directory, to measure the descent from.
    root: PathBuf,
}

impl OutStyle {
    /// Derive the style from the start directory as given, before absolutizing.
    fn new(given: &str, root: PathBuf) -> Self {
        let prefix = if Path::new(given).is_absolute() {
            None
        } else {
            // `"."` and `""` mean "no prefix": the descent alone is already the
            // path `find .`-style output would show, without a `./` on every
            // entry. A trailing slash would double up on the join.
            let trimmed = given.trim_end_matches('/');
            match trimmed {
                "" | "." => Some(String::new()),
                other => Some(other.to_string()),
            }
        };
        Self { prefix, root }
    }

    /// A style that reports whatever it is handed, verbatim and absolute. For
    /// the callers that have no start directory to take a spelling from.
    fn absolute() -> Self {
        Self {
            prefix: None,
            root: PathBuf::new(),
        }
    }

    /// Render one absolute path for the script, lossily UTF-8.
    ///
    /// The start directory itself renders as the prefix (or `"."` when there is
    /// none), so every returned path is usable as one rather than empty.
    fn render(&self, path: &Path) -> String {
        let Some(prefix) = self.prefix.as_deref() else {
            return path.to_string_lossy().into_owned();
        };
        let descent = path.strip_prefix(&self.root).unwrap_or(path);
        if descent.as_os_str().is_empty() {
            return if prefix.is_empty() {
                ".".to_string()
            } else {
                prefix.to_string()
            };
        }
        let descent = descent.to_string_lossy();
        if prefix.is_empty() {
            descent.into_owned()
        } else {
            format!("{prefix}/{descent}")
        }
    }
}

/// Recursively copy the directory `from` to `to`, merging into `to` if it
/// already exists.
///
/// Symlinks are copied as what they stand for: a link to a file contributes the
/// file's contents, a link to a directory contributes that directory's tree —
/// including one pointing outside the work directory, which is the point of the
/// host filesystem.
///
/// `ancestors` carries the canonical directories on the current branch, so a
/// link pointing back up its own ancestry is not descended into a second time.
fn copy_tree(from: &Path, to: &Path) -> std::result::Result<(), String> {
    fn inner(
        from: &Path,
        to: &Path,
        ancestors: &mut Vec<PathBuf>,
    ) -> std::result::Result<(), String> {
        std::fs::create_dir_all(to).map_err(|e| format!("mkdir {}: {e}", to.display()))?;
        let entries =
            std::fs::read_dir(from).map_err(|e| format!("list {}: {e}", from.display()))?;
        for entry in entries.flatten() {
            let src = entry.path();
            let Some(name) = src.file_name() else {
                continue;
            };
            let dst = to.join(name);

            // Canonicalizing resolves the link; a dangling one is skipped,
            // since there is nothing to copy.
            let Ok(canonical) = src.canonicalize() else {
                continue;
            };
            let meta = std::fs::metadata(&canonical)
                .map_err(|e| format!("stat {}: {e}", canonical.display()))?;
            if meta.is_dir() {
                if ancestors.contains(&canonical) {
                    continue;
                }
                // Never descend into the destination itself. A `to` inside `from`
                // is refused up front, but a symlink can still point at it from
                // deeper in the tree, and copying it into itself would not end.
                if to.canonicalize().is_ok_and(|t| canonical.starts_with(&t)) {
                    continue;
                }
                ancestors.push(canonical.clone());
                inner(&canonical, &dst, ancestors)?;
                ancestors.pop();
            } else {
                std::fs::copy(&canonical, &dst).map(|_| ()).map_err(|e| {
                    format!("copy {} -> {}: {e}", canonical.display(), dst.display())
                })?;
            }
        }
        Ok(())
    }

    let mut ancestors = vec![from.canonicalize().unwrap_or_else(|_| from.to_path_buf())];
    inner(from, to, &mut ancestors)
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
/// a file, a link to a directory is descended into — wherever it leads. It keeps
/// *its own* path though, so a link and its target are two entries, the way `ls`
/// shows them, rather than the same path twice.
///
/// Hidden files are skipped and hidden directories are not descended into
/// (`fs.glob` never traverses hidden entries; use `fs.finder(..):hidden()`).
fn walk(dir: &Path, ancestors: &mut Vec<PathBuf>, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if path.file_name().is_some_and(is_hidden) {
            continue;
        }
        // The canonical form is used for the kind and the cycle check only --
        // what gets reported is `path`, so a link keeps its own name.
        let Ok(canonical) = path.canonicalize() else {
            continue;
        };
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
                walk(&path, ancestors, out);
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
    /// The directory this step describes, as an absolute path.
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
/// Symlink handling matches [`walk`] -- a link counts as what it points at and
/// is listed under its own name. Hidden entries are included: `fs.walk` reports
/// a directory's contents rather than filtering them, and a caller that wants
/// them gone can say so.
fn walk_tree(dir: &Path, ancestors: &mut Vec<PathBuf>, out: &mut Vec<WalkStep>) {
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
        walk_tree(&path, ancestors, out);
        ancestors.pop();
    }
}

/// The host filesystem as a finder [`Source`]: the walk starts wherever the
/// script pointed it and every path is rendered in the spelling that start
/// directory established. Nothing is refused — there is no boundary here — so
/// [`render`](Source::render) is total.
pub struct HostSource {
    start: PathBuf,
    out: OutStyle,
}

impl finder::Source for HostSource {
    fn start(&self) -> PathBuf {
        self.start.clone()
    }

    fn render(&self, path: &Path) -> Option<String> {
        Some(self.out.render(path))
    }
}

/// The host `fs.finder`'s handle.
///
/// A newtype rather than an alias: `UserData` is implemented here and the
/// generic finder lives in another module, so the orphan rule needs a local
/// type. Cloning shares the builder state, as the inner finder does.
#[derive(Clone)]
pub struct Finder(finder::Finder<HostSource>);

impl UserData for Finder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        finder::add_methods_with_follow(methods, |f: &Finder| &f.0);
    }
}

/// Register the host-filesystem `fs` module on `lua`.
///
/// No root and no state: every function resolves through `std::fs`, and a
/// relative path lands in the work directory because the process cwd is already
/// there.
pub fn register(lua: &Lua) -> Result<()> {
    let module = lua.create_table()?;

    // fs.cwd() -> String
    // The work directory, absolute. This is the frame every other function
    // here resolves against, so it is the one path that cannot be spelled
    // relatively -- `out_path` would render it `"."`, which is what a script
    // asking for it already has. Deliberately absent from the rooted VFS,
    // where the host location is not a script's business.
    module.set(
        "cwd",
        lua.create_function(|_, ()| -> Result<String> {
            let dir =
                std::env::current_dir().map_err(|e| mlua::Error::runtime(format!("cwd: {e}")))?;
            Ok(dir.to_string_lossy().into_owned())
        })?,
    )?;

    // fs.canonicalize(path) -> String
    // The absolute, symlink-resolved, `.`/`..`-collapsed form of `path`.
    // Errors if the path does not exist -- resolving a symlink means
    // reading it, so there is nothing to answer for a path that is not
    // there. Absolute on purpose, unlike the traversals: the point of
    // asking is to compare two locations (against `fs.cwd()`, say), and a
    // work-directory-relative rendering would make the comparison the
    // caller's problem again. Host-only for the same reason `fs.cwd()` is:
    // under the rooted model the root is a boundary, not a location.
    module.set(
        "canonicalize",
        lua.create_function(|_, path: String| -> Result<String> {
            let resolved = std::fs::canonicalize(&path)
                .map_err(|e| mlua::Error::runtime(format!("canonicalize {path}: {e}")))?;
            Ok(resolved.to_string_lossy().into_owned())
        })?,
    )?;

    // fs.readlink(path) -> String
    // The link's target *verbatim*, one hop, unresolved: a relative target
    // stays relative (to the link's own directory, not the work directory) and
    // a dangling one still reads. `fs.canonicalize` is the other question --
    // follow every hop and give an absolute answer. Errors if `path` is not a
    // symlink, there being no target to name.
    module.set(
        "readlink",
        lua.create_function(|_, path: String| -> Result<String> {
            let target = std::fs::read_link(&path)
                .map_err(|e| mlua::Error::runtime(format!("readlink {path}: {e}")))?;
            Ok(target.to_string_lossy().into_owned())
        })?,
    )?;

    // fs.symlink(target, path)
    // Creates `path` as a symlink to `target`, argument order matching `ln -s`
    // (and `fs.copy`/`fs.rename`: source first). `target` is stored verbatim
    // and never resolved, so it may be relative or not yet exist -- a dangling
    // link is a legitimate thing to create, and checking would break the
    // create-in-any-order case. Parents of `path` are created, as `fs.write`
    // does.
    //
    // Unix-only: `std::fs` has no portable symlink, and the Windows form needs
    // to know file-vs-directory up front (a distinction this signature does not
    // carry) plus a privilege the process may not hold. Absent rather than
    // erroring, so `if fs.symlink then` feature-tests like the capability
    // globals do.
    #[cfg(unix)]
    module.set(
        "symlink",
        lua.create_function(|_, (target, path): (String, String)| -> Result<()> {
            if let Some(parent) = Path::new(&path).parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent).map_err(|e| {
                    mlua::Error::runtime(format!("symlink {path}: {}: {e}", parent.display()))
                })?;
            }
            std::os::unix::fs::symlink(&target, &path)
                .map_err(|e| mlua::Error::runtime(format!("symlink {path} -> {target}: {e}")))
        })?,
    )?;

    // fs.list(dir) -> {String}
    // Entries (files and directories) directly under `dir`. Sorted. Spelled
    // the way `dir` was: a relative `dir` gives `dir/name`, an absolute one
    // gives absolute paths.
    module.set(
        "list",
        lua.create_function(|_, dir: String| -> Result<Vec<String>> {
            let abs = absolutize(Path::new(&dir));
            let style = OutStyle::new(&dir, abs.clone());
            let entries = std::fs::read_dir(&abs)
                .map_err(|e| mlua::Error::runtime(format!("list {}: {e}", abs.display())))?;
            let mut out: Vec<String> = Vec::new();
            for entry in entries.flatten() {
                out.push(style.render(&entry.path()));
            }
            out.sort();
            Ok(out)
        })?,
    )?;

    // fs.walk([dir]) -> {{path=String, dirs={String}, files={String}}}
    // Top-down, one entry per directory visited, as `os.walk` yields
    // `(dirpath, dirnames, filenames)`. `dir` defaults to the work directory.
    module.set(
        "walk",
        lua.create_function(|lua, dir: Option<String>| -> Result<Vec<Table>> {
            let given = dir.as_deref().unwrap_or(".");
            let start = absolutize(Path::new(given));
            let style = OutStyle::new(given, start.clone());
            let mut steps: Vec<WalkStep> = Vec::new();
            let mut ancestors: Vec<PathBuf> =
                vec![start.canonicalize().unwrap_or_else(|_| start.clone())];
            walk_tree(&start, &mut ancestors, &mut steps);

            let mut out = Vec::with_capacity(steps.len());
            for step in steps {
                let t = lua.create_table()?;
                t.set("path", style.render(&step.dir))?;
                t.set("dirs", step.dirs)?;
                t.set("files", step.files)?;
                out.push(t);
            }
            Ok(out)
        })?,
    )?;

    // fs.glob(pattern) -> {String}
    // Regular files matching a glob, sorted. Matching happens on the absolute
    // path (a relative `pattern` is taken against the work directory), and the
    // results are spelled the way `pattern` was: a relative pattern gives
    // work-directory-relative paths, an absolute one gives absolute paths. So
    // `fs.glob("sub/*.txt")` answers in the same terms it was asked in.
    // Hidden files and dot-directories are always excluded; use
    // `fs.finder(..):hidden()` to include them.
    module.set(
        "glob",
        lua.create_function(|_, pattern: String| -> Result<Vec<String>> {
            let pattern_abs = absolutize(Path::new(&pattern));
            let matcher = build_matcher(&pattern_abs.to_string_lossy())?;
            let start = absolutize(Path::new("."));
            // The work directory is the walk root either way; only the spelling
            // of the answer follows the pattern.
            let style = if Path::new(&pattern).is_absolute() {
                OutStyle::absolute()
            } else {
                OutStyle::new(".", start.clone())
            };
            let mut files: Vec<PathBuf> = Vec::new();
            let mut ancestors: Vec<PathBuf> =
                vec![start.canonicalize().unwrap_or_else(|_| start.clone())];
            walk(&start, &mut ancestors, &mut files);
            let mut out: Vec<String> = files
                .iter()
                .filter(|p| matcher.is_match(p))
                .map(|p| style.render(p))
                .collect();
            out.sort();
            Ok(out)
        })?,
    )?;

    // fs.finder([dir]) -> Finder
    // Start an `fd`-like search rooted at `dir` (default: the work directory).
    // The start directory is the only argument -- there is no seed pattern, so
    // an unfiltered finder means "everything under here" and each condition is
    // added as its own named filter. Chain `:name()`/`:path()`/`:ext()`,
    // `:exclude()`, `:files()`/`:dirs()`/`:type()`, `:depth()`/`:min_depth()`/
    // `:max_depth()`, `:hidden()`/`:no_ignore()`/`:unrestricted()`, `:follow()`,
    // `:sort()`/`:limit()`, then a terminal: `:find()`, `:count()`, `:any()`, or
    // -- searching the matched files' contents -- `:grep()`/`:grep_files()`.
    // Results are spelled the way `dir` was, as every traversal here is.
    module.set(
        "finder",
        lua.create_function(|_, dir: Option<String>| -> Result<Finder> {
            let given = dir.as_deref().unwrap_or(".");
            let start = absolutize(Path::new(given));
            Ok(Finder(finder::Finder::new(HostSource {
                out: OutStyle::new(given, start.clone()),
                start,
            })))
        })?,
    )?;

    // fs.read(path) -> String
    module.set(
        "read",
        lua.create_function(|_, path: String| -> Result<String> {
            std::fs::read_to_string(&path)
                .map_err(|e| mlua::Error::runtime(format!("read {path}: {e}")))
        })?,
    )?;

    // fs.write(path, content)
    // Written now, parents created now.
    module.set(
        "write",
        lua.create_function(|_, (path, content): (String, String)| -> Result<()> {
            write_now(Path::new(&path), &content).map_err(mlua::Error::runtime)
        })?,
    )?;

    // fs.write_new(path[, content]) -> bool
    // Create `path` only if it does not exist, atomically: `O_CREAT|O_EXCL`,
    // one syscall, so two processes racing to it cannot both win. `true` if
    // this call created it, `false` if it was already there. An existing
    // file is the *answer*, not an error -- losing the race is the outcome
    // the caller asked about, and raising would make every use site a
    // `pcall`. Anything else (a missing parent that could not be made, a
    // permission error) still raises.
    //
    // `content` is optional and defaults to empty, so this is both "claim
    // this name" and "write this file unless it exists". Parents are
    // created, as `fs.write` does.
    //
    // This is the lock primitive: `fs.write_new` takes it and `fs.remove`
    // releases it. There is deliberately no lock *object* -- no handle, no
    // release-on-GC, no wait/timeout, no staleness rule. Which of those a
    // caller wants is not general (a stale lock is only detectable against
    // something this module cannot know: a pid, a host, a deadline written
    // into the file), and a handle released by the garbage collector would
    // promise a lifetime Luau does not guarantee. Waiting is
    // `task.wait` in a loop; recording who holds it is what `content` is
    // for.
    //
    // Host-only for the reason the other direct writers are: "created now,
    // atomically" is the one thing a buffered write cannot express -- the
    // whole point is the answer arriving before the script continues.
    module.set(
        "write_new",
        lua.create_function(
            |_, (path, content): (String, Option<String>)| -> Result<bool> {
                let p = Path::new(&path);
                if let Some(parent) = p.parent()
                    && !parent.as_os_str().is_empty()
                {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        mlua::Error::runtime(format!("write_new {path}: {}: {e}", parent.display()))
                    })?;
                }
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(p)
                {
                    Ok(mut f) => {
                        use std::io::Write;
                        f.write_all(content.unwrap_or_default().as_bytes())
                            .map_err(|e| mlua::Error::runtime(format!("write_new {path}: {e}")))?;
                        Ok(true)
                    }
                    // Only an existing *file* is "you lost the race". A
                    // directory (or a socket, or a dangling symlink, all of
                    // which `AlreadyExists` covers on some platforms) is a
                    // name that `fs.remove` can never release, so reporting
                    // `false` would send a waiting caller into a loop that
                    // cannot end. Raise instead, and say what is in the way.
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        if p.symlink_metadata().map(|m| m.is_file()).unwrap_or(false) {
                            Ok(false)
                        } else {
                            Err(mlua::Error::runtime(format!(
                                "write_new {path}: exists and is not a regular file"
                            )))
                        }
                    }
                    Err(e) => Err(mlua::Error::runtime(format!("write_new {path}: {e}"))),
                }
            },
        )?,
    )?;

    // fs.remove(path)
    // Removed now; an absent file is a no-op.
    module.set(
        "remove",
        lua.create_function(|_, path: String| -> Result<()> {
            remove_now(Path::new(&path)).map_err(mlua::Error::runtime)
        })?,
    )?;

    // fs.mkdir(path)
    // Creates parents too, and an existing directory is success, not an
    // error -- the state the caller asked for is the state they get.
    module.set(
        "mkdir",
        lua.create_function(|_, path: String| -> Result<()> {
            std::fs::create_dir_all(&path)
                .map_err(|e| mlua::Error::runtime(format!("mkdir {path}: {e}")))
        })?,
    )?;

    // fs.rmdir(path)
    // Removes a directory and everything under it. An absent path is a
    // no-op, matching `fs.remove`.
    module.set(
        "rmdir",
        lua.create_function(|_, path: String| -> Result<()> {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(mlua::Error::runtime(format!("rmdir {path}: {e}"))),
            }
        })?,
    )?;

    // fs.rename(from, to)
    // Parents of `to` are created, as `fs.write` would.
    module.set(
        "rename",
        lua.create_function(|_, (from, to): (String, String)| -> Result<()> {
            if let Some(parent) = Path::new(&to).parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    mlua::Error::runtime(format!("mkdir {}: {e}", parent.display()))
                })?;
            }
            std::fs::rename(&from, &to)
                .map_err(|e| mlua::Error::runtime(format!("rename {from} -> {to}: {e}")))
        })?,
    )?;

    // fs.copy(from, to)
    // A file copies as a file, a directory copies as a tree. Parents of
    // `to` are created either way, as `fs.write` would. A symlink is copied
    // as what it stands for -- its target's contents, not the link -- which
    // is the same reading `fs.finder` and the rest of the module give it.
    module.set(
        "copy",
        lua.create_function(|_, (from, to): (String, String)| -> Result<()> {
            let (from_abs, to_abs) = (absolutize(Path::new(&from)), absolutize(Path::new(&to)));
            if let Some(parent) = to_abs.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    mlua::Error::runtime(format!("mkdir {}: {e}", parent.display()))
                })?;
            }
            // `metadata` follows the link, so a symlinked directory copies
            // as the tree it points at.
            let is_dir = std::fs::metadata(&from_abs)
                .map(|m| m.is_dir())
                .unwrap_or(false);
            if is_dir {
                // Copying a directory into itself never terminates: the
                // destination is created inside the source, so the walk
                // keeps finding more to copy. Refuse rather than run away.
                if to_abs.starts_with(&from_abs) {
                    return Err(mlua::Error::runtime(format!(
                        "copy {} -> {}: destination is inside the source",
                        from_abs.display(),
                        to_abs.display()
                    )));
                }
                copy_tree(&from_abs, &to_abs).map_err(mlua::Error::runtime)
            } else {
                std::fs::copy(&from_abs, &to_abs).map(|_| ()).map_err(|e| {
                    mlua::Error::runtime(format!(
                        "copy {} -> {}: {e}",
                        from_abs.display(),
                        to_abs.display()
                    ))
                })
            }
        })?,
    )?;

    // fs.exists(path) -> bool   (follows links: a dangling link is not a file)
    module.set(
        "exists",
        lua.create_function(|_, path: String| -> Result<bool> {
            Ok(Path::new(&path).metadata().is_ok())
        })?,
    )?;

    // fs.is_file(path) -> bool  (follows links, like the traversals: a link to
    // a file *is* a file. `fs.is_symlink` asks the other question.)
    module.set(
        "is_file",
        lua.create_function(|_, path: String| -> Result<bool> {
            Ok(Path::new(&path)
                .metadata()
                .map(|m| m.is_file())
                .unwrap_or(false))
        })?,
    )?;

    // fs.is_dir(path) -> bool   (follows links)
    module.set(
        "is_dir",
        lua.create_function(|_, path: String| -> Result<bool> {
            Ok(Path::new(&path)
                .metadata()
                .map(|m| m.is_dir())
                .unwrap_or(false))
        })?,
    )?;

    // fs.is_symlink(path) -> bool  (the only predicate that does *not* follow:
    // it asks about the entry itself, so it is true for a dangling link too.)
    module.set(
        "is_symlink",
        lua.create_function(|_, path: String| -> Result<bool> {
            Ok(Path::new(&path)
                .symlink_metadata()
                .map(|m| m.is_symlink())
                .unwrap_or(false))
        })?,
    )?;

    lua.globals().set("fs", module)?;
    Ok(())
}
