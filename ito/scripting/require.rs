//! ito's own Luau `require` resolver.
//!
//! Replaces mlua's default [`FsRequirer`] to enforce ito's rules
//! (`docs/src/luau/require.md`):
//!
//! - **Confinement.** Imports are limited to the directory tree of the
//!   *entry* script — the `root`. `../`-relative paths may not walk above
//!   it, and neither may a symlink: the root is canonicalized, and a module
//!   file is canonicalized before it is accepted, so a link leading out of the
//!   tree is an error (and aliases such as macOS's `/tmp` -> `/private/tmp`
//!   do not trip the textual check). For `ito <path>` the root is the
//!   directory containing `<path>`. This is independent of the `fs` VFS root,
//!   which constrains file *operations*, not imports.
//! - **Relative to the requirer.** Each `require` resolves relative to the
//!   directory of the file that calls it, same as upstream Luau. A module is
//!   named by its real (canonical) file, so a symlinked module resolves beside
//!   the target and is cached once however it was reached.
//! - **Candidate order.** For a path segment `foo`:
//!     1. if `foo` already ends in `.lua`/`.luau`, only that exact file;
//!     2. otherwise `foo.luau` then `foo.lua` (the extension is *appended*, so
//!        `config.prod` names `config.prod.luau`);
//!     3. fallback: `foo/init.luau` then `foo/init.lua`.
//!   *All* candidates are checked: more than one match is an error that
//!   names the conflicting files; no match is an error that lists the
//!   paths that were checked. At the root itself only `init.*` counts — the
//!   root's sibling `<root>.lua` is not a candidate. Resolution problems
//!   surface when the module is *loaded*, not while walking intermediate
//!   segments, so an ambiguous intermediate name no longer masks the real
//!   target.
//! - **Aliases.** `@name/rest` resolves `name` through the `aliases` table
//!   of the nearest `.luaurc`, as upstream Luau does. Two deviations keep
//!   the confinement guarantee intact: the search for `.luaurc` stops at
//!   the root rather than inheriting to the filesystem root, so a config
//!   outside the tree cannot influence a confined script; and an alias
//!   target is bounded like any other path, so an alias cannot be used to
//!   escape. `@self` names the requiring module's own path, which is how a
//!   `foo/init.luau` addresses the directory it stands in for.
//!
//! [`FsRequirer`]: mlua::luau::FsRequirer

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::Result as IoResult;
use std::path::{Component, Path, PathBuf};
use std::result::Result as StdResult;

use mlua::luau::{NavigateError, Require};
use mlua::{Function, Lua, Result};

/// The chunk-name prefix Luau uses to mark filesystem-backed chunks.
const CHUNK_PREFIX: &str = "@";

/// Extensions tried (in order) when a segment has no explicit extension.
const FILE_EXTENSIONS: &[&str] = &["luau", "lua"];

/// The config file consulted for alias definitions.
const CONFIG_FILENAME: &str = ".luaurc";

/// What a navigated path resolved to: a single module file, a bare directory
/// (`None`, a valid *intermediate* step), or the message explaining why not.
type Resolution = StdResult<Option<PathBuf>, String>;

/// ito's `require` navigator. See the module docs for the rules.
#[derive(Debug)]
pub struct ItoRequirer {
    /// Confinement boundary: no navigation may resolve above this path.
    /// Canonical where the filesystem allows.
    root: PathBuf,
    /// Absolute logical path to the current module (not yet mapped to a file).
    abs_path: PathBuf,
    /// The outcome of resolving `abs_path`, if a module was navigated to.
    resolved: Option<Resolution>,
}

impl ItoRequirer {
    /// Create a requirer confined to `root` (the entry script's directory).
    pub fn new(root: PathBuf) -> Self {
        // Canonical, so a symlink-resolved module file can be compared to it.
        let root = root
            .canonicalize()
            .unwrap_or_else(|_| normalize_path(&root));
        Self {
            root,
            abs_path: PathBuf::new(),
            resolved: None,
        }
    }

    /// True if `path` is inside (or equal to) the confinement root.
    fn within_root(&self, path: &Path) -> bool {
        path.starts_with(&self.root)
    }

    /// The config file for the current context, if that context is a directory
    /// inside the root.
    ///
    /// Luau asks about a *directory* during alias resolution, so a context
    /// still pointing at a module file has no config of its own.
    fn config_path(&self) -> Option<PathBuf> {
        (self.within_root(&self.abs_path) && self.abs_path.is_dir())
            .then(|| self.abs_path.join(CONFIG_FILENAME))
    }

    /// Move the context to `path`, rejecting anything outside the root.
    ///
    /// Used by alias jumps, which relocate the context wholesale rather than
    /// stepping through it.
    fn jump_to(&mut self, path: PathBuf, what: &str) -> StdResult<(), NavigateError> {
        let path = normalize_path(&path);
        if !self.within_root(&path) {
            return Err(NavigateError::Other(mlua::Error::runtime(format!(
                "{what} escapes the script root '{}': '{}'",
                self.root.display(),
                path.display()
            ))));
        }
        self.resolved = Some(self.resolve_module(&path));
        self.abs_path = path;
        Ok(())
    }

    /// Resolve a logical module path to a physical file following ito's
    /// candidate order, or classify why it could not.
    ///
    /// `Ok(Some(file))` — a single module file matched (canonical path).
    /// `Ok(None)`       — `path` is an existing directory with no `init`
    ///                    module (a valid *intermediate* directory).
    /// `Err(..)`        — ambiguous (names the files), not found (lists the
    ///                    checked paths) or outside the root.
    fn resolve_module(&self, path: &Path) -> Resolution {
        let mut checked: Vec<PathBuf> = Vec::new();
        let mut matches: Vec<PathBuf> = Vec::new();

        let has_lua_ext = matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("lua") | Some("luau")
        );

        if has_lua_ext {
            // The segment carries an explicit extension: only that exact file
            // (no extension search, no `init` fallback).
            checked.push(path.to_path_buf());
            if path.is_file() {
                matches.push(path.to_path_buf());
            }
        } else {
            // At the root only `init.*` counts; `<root>.lua` beside it is
            // outside the tree.
            if path != self.root {
                // `foo.luau`, then `foo.lua`; appended, not substituted, so a
                // dotted name keeps its dot.
                for ext in FILE_EXTENSIONS {
                    let mut name = OsString::from(path.as_os_str());
                    name.push(format!(".{ext}"));
                    let candidate = PathBuf::from(name);
                    checked.push(candidate.clone());
                    if candidate.is_file() {
                        matches.push(candidate);
                    }
                }
            }

            // Fallback: a directory module `foo/init.{luau,lua}`.
            if path.is_dir() {
                for ext in FILE_EXTENSIONS {
                    let candidate = path.join(format!("init.{ext}"));
                    checked.push(candidate.clone());
                    if candidate.is_file() {
                        matches.push(candidate);
                    }
                }
            }
        }

        match matches.len() {
            1 => {
                // Backstop: the file's real location must be inside the root,
                // whatever path (or symlink) led to it.
                let file = matches.pop().unwrap();
                let real = file
                    .canonicalize()
                    .map_err(|e| format!("could not resolve module '{}': {e}", file.display()))?;
                if !self.within_root(&real) {
                    return Err(format!(
                        "module '{}' resolves outside the script root '{}': '{}'",
                        file.display(),
                        self.root.display(),
                        real.display()
                    ));
                }
                Ok(Some(real))
            }
            0 => {
                // A bare existing directory with no module is a valid
                // intermediate path (e.g. `sub` in `./sub/lib`).
                if !has_lua_ext && path.is_dir() {
                    return Ok(None);
                }
                let listed = checked
                    .iter()
                    .map(|p| format!("  {}", p.display()))
                    .collect::<Vec<_>>()
                    .join("\n");
                Err(format!(
                    "no module found for '{}'; checked:\n{listed}",
                    path.display()
                ))
            }
            _ => {
                let listed = matches
                    .iter()
                    .map(|p| format!("  {}", p.display()))
                    .collect::<Vec<_>>()
                    .join("\n");
                Err(format!(
                    "ambiguous module for '{}'; multiple files match:\n{listed}",
                    path.display()
                ))
            }
        }
    }
}

impl Require for ItoRequirer {
    fn is_require_allowed(&self, chunk_name: &str) -> bool {
        chunk_name.starts_with(CHUNK_PREFIX)
    }

    fn reset(&mut self, chunk_name: &str) -> StdResult<(), NavigateError> {
        // The requirer chunk name is `@<abs path>` (the entry script's has no
        // extension); the navigator immediately calls `to_parent` to move from
        // this file to its directory before applying the require path's
        // components.
        let stripped = chunk_name
            .strip_prefix(CHUNK_PREFIX)
            .ok_or(NavigateError::NotFound)?;
        let mut abs = normalize_path(Path::new(normalize_chunk_name(stripped)));
        // Module chunks keep their extension; `@self` names the path without it.
        if matches!(abs.extension().and_then(|s| s.to_str()), Some("lua" | "luau")) {
            abs.set_extension("");
        }
        // A required `pkg/init.luau` stands in for the directory `pkg`, so its
        // relative paths start beside `pkg` (`@self` names the inside). The
        // entry script is not a module: an entry `init.luau` sits *in* the root
        // and resolves `./` within it.
        if abs.file_stem().is_some_and(|s| s == "init") && abs.parent() != Some(&self.root) {
            abs.pop();
        }
        self.abs_path = abs;
        // The requirer is the calling chunk itself; no module file to resolve.
        self.resolved = None;
        Ok(())
    }

    fn jump_to_alias(&mut self, path: &str) -> StdResult<(), NavigateError> {
        self.jump_to(PathBuf::from(path), "alias")
    }

    fn to_parent(&mut self) -> StdResult<(), NavigateError> {
        let mut abs_path = self.abs_path.clone();
        if !abs_path.pop() {
            return Err(NavigateError::NotFound);
        }
        // Confinement: never resolve above the entry script's root. The
        // first (automatic) `to_parent` moves from the entry file to its
        // own directory — that is the root itself, so it is allowed; only
        // `../` components that climb past the root are rejected.
        //
        // The root boundary is a *recoverable* `NotFound`, not an error:
        // Luau also walks up with `to_parent` when it is hunting for a
        // config file to resolve an alias, and treats the failure to climb
        // as "the search ends here". Reporting an error instead aborts that
        // search, which breaks every alias used near the root — `@self`
        // among them.
        if !self.within_root(&abs_path) {
            return Err(NavigateError::NotFound);
        }
        self.resolved = Some(self.resolve_module(&abs_path));
        self.abs_path = abs_path;
        Ok(())
    }

    fn to_child(&mut self, name: &str) -> StdResult<(), NavigateError> {
        let abs_path = self.abs_path.join(name);
        if !self.within_root(&abs_path) {
            return Err(NavigateError::Other(mlua::Error::runtime(format!(
                "require escapes the script root '{}': '{}'",
                self.root.display(),
                abs_path.display()
            ))));
        }
        self.resolved = Some(self.resolve_module(&abs_path));
        self.abs_path = abs_path;
        Ok(())
    }

    fn has_module(&self) -> bool {
        match &self.resolved {
            Some(Ok(Some(file))) => file.is_file(),
            // Not found / ambiguous / escaping: claim a module so the loader
            // can raise the descriptive error instead of a generic one.
            Some(Err(_)) => true,
            _ => false,
        }
    }

    fn cache_key(&self) -> String {
        match &self.resolved {
            Some(Ok(Some(file))) => file.display().to_string(),
            _ => self.abs_path.display().to_string(),
        }
    }

    fn has_config(&self) -> bool {
        // Luau resolves an alias by walking up from the requiring module and
        // asking at each level; `to_parent` already refuses to climb past the
        // root, so the search cannot reach a config outside the tree.
        self.config_path().is_some_and(|p| p.is_file())
    }

    fn config(&self) -> IoResult<Vec<u8>> {
        let path = self.config_path().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no config in this context")
        })?;
        std::fs::read(path)
    }

    fn loader(&self, lua: &Lua) -> Result<Function> {
        match &self.resolved {
            // The real file, extension kept, so tracebacks name something
            // openable and `fs.require_dir()` is the file's own directory.
            Some(Ok(Some(file))) => lua
                .load(file.as_path())
                .set_name(format!("{CHUNK_PREFIX}{}", file.display()))
                .into_function(),
            Some(Err(msg)) => Err(mlua::Error::runtime(msg.clone())),
            _ => Err(mlua::Error::runtime(format!(
                "no module found for '{}'",
                self.abs_path.display()
            ))),
        }
    }
}

/// Strip a trailing `:<line>` suffix that Luau may append to chunk names.
fn normalize_chunk_name(chunk_name: &str) -> &str {
    if let Some((path, line)) = chunk_name.rsplit_once(':') {
        if line.parse::<u32>().is_ok() {
            return path;
        }
    }
    chunk_name
}

/// Collapse `.`/`..`/redundant separators without touching the filesystem.
/// Mirrors `FsRequirer::normalize_path`.
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = VecDeque::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(..) | Component::RootDir => components.push_back(comp),
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(components.back(), None | Some(Component::ParentDir)) {
                    components.push_back(Component::ParentDir);
                } else if matches!(components.back(), Some(Component::Normal(..))) {
                    components.pop_back();
                }
            }
            Component::Normal(..) => components.push_back(comp),
        }
    }
    components.into_iter().collect()
}

/// Install ito's `require` as the global, confined to `root`.
pub fn register(lua: &Lua, root: PathBuf) -> Result<()> {
    let require = lua.create_require_function(ItoRequirer::new(root))?;
    lua.globals().raw_set("require", require)?;
    Ok(())
}
