//! ito's own Luau `require` resolver.
//!
//! Replaces mlua's default [`FsRequirer`] to enforce ito's rules
//! (`docs/src/lua/require.md`):
//!
//! - **Confinement.** Imports are limited to the directory tree of the
//!   *entry* script — the `root`. `../`-relative paths may not walk above
//!   it. For `ito <path>` the root is the directory containing `<path>`;
//!   for `-s`/`--script` (inline) or `ito -` (stdin, no script file) it is
//!   the working directory (the `-C` root). This is independent of the
//!   `fs` VFS root, which constrains file *operations*, not imports.
//! - **Relative to the requirer.** Each `require` resolves relative to the
//!   directory of the file that calls it, same as upstream Luau.
//! - **Candidate order.** For a path segment `foo`:
//!     1. if `foo` already ends in `.lua`/`.luau`, only that exact file;
//!     2. otherwise `foo.luau` then `foo.lua`;
//!     3. fallback: `foo/init.luau` then `foo/init.lua`.
//!   *All* candidates are checked: more than one match is an error that
//!   names the conflicting files; no match is an error that lists the
//!   paths that were checked.
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

/// ito's `require` navigator. See the module docs for the rules.
#[derive(Debug)]
pub struct ItoRequirer {
    /// Confinement boundary: no navigation may resolve above this path.
    root: PathBuf,
    /// Absolute logical path to the current module (not yet mapped to a file).
    abs_path: PathBuf,
    /// Path relative to the requirer, used only for the chunk name.
    rel_path: PathBuf,
    /// The physical file backing the current module, if one resolved.
    resolved_path: Option<PathBuf>,
}

impl ItoRequirer {
    /// Create a requirer confined to `root` (the entry script's directory,
    /// or the working directory for inline/stdin scripts).
    pub fn new(root: PathBuf) -> Self {
        Self {
            root: normalize_path(&root),
            abs_path: PathBuf::new(),
            rel_path: PathBuf::new(),
            resolved_path: None,
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
        let resolved = Self::resolve_module(&path)?;
        self.abs_path = path.clone();
        self.rel_path = path;
        self.resolved_path = resolved;
        Ok(())
    }

    /// Resolve a logical module path to a physical file following ito's
    /// candidate order, or classify why it could not.
    ///
    /// `Ok(Some(file))` — a single module file matched.
    /// `Ok(None)`       — `path` is an existing directory with no `init`
    ///                    module (a valid *intermediate* directory).
    /// `Err(..)`        — ambiguous (names the files) or not found (lists
    ///                    the checked paths).
    fn resolve_module(path: &Path) -> StdResult<Option<PathBuf>, NavigateError> {
        let mut checked: Vec<PathBuf> = Vec::new();
        let mut matches: Vec<PathBuf> = Vec::new();

        let has_lua_ext = matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("lua") | Some("luau")
        );

        if has_lua_ext {
            // The segment carries an explicit extension: only that file.
            checked.push(path.to_path_buf());
            if path.is_file() {
                matches.push(path.to_path_buf());
            }
        } else {
            // `foo.luau`, then `foo.lua`.
            for ext in FILE_EXTENSIONS {
                let candidate = path.with_extension(ext);
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

        match matches.len() {
            1 => Ok(Some(matches.pop().unwrap())),
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
                Err(NavigateError::Other(mlua::Error::runtime(format!(
                    "no module found for '{}'; checked:\n{listed}",
                    path.display()
                ))))
            }
            _ => {
                let listed = matches
                    .iter()
                    .map(|p| format!("  {}", p.display()))
                    .collect::<Vec<_>>()
                    .join("\n");
                Err(NavigateError::Other(mlua::Error::runtime(format!(
                    "ambiguous module for '{}'; multiple files match:\n{listed}",
                    path.display()
                ))))
            }
        }
    }
}

impl Require for ItoRequirer {
    fn is_require_allowed(&self, chunk_name: &str) -> bool {
        chunk_name.starts_with(CHUNK_PREFIX)
    }

    fn reset(&mut self, chunk_name: &str) -> StdResult<(), NavigateError> {
        // The requirer chunk name is `@<abs path minus extension>`; the
        // navigator immediately calls `to_parent` to move from this file to
        // its directory before applying the require path's components.
        let stripped = chunk_name
            .strip_prefix(CHUNK_PREFIX)
            .ok_or(NavigateError::NotFound)?;
        let abs = normalize_path(Path::new(normalize_chunk_name(stripped)));
        self.abs_path = abs.clone();
        self.rel_path = abs;
        // The requirer is the entry chunk itself; no module file to resolve.
        self.resolved_path = None;
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
        let mut rel_parent = self.rel_path.clone();
        rel_parent.pop();
        let resolved = Self::resolve_module(&abs_path)?;
        self.abs_path = abs_path;
        self.rel_path = normalize_path(&rel_parent);
        self.resolved_path = resolved;
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
        let resolved = Self::resolve_module(&abs_path)?;
        self.rel_path = self.rel_path.join(name);
        self.abs_path = abs_path;
        self.resolved_path = resolved;
        Ok(())
    }

    fn has_module(&self) -> bool {
        self.resolved_path
            .as_deref()
            .map(Path::is_file)
            .unwrap_or(false)
    }

    fn cache_key(&self) -> String {
        self.resolved_path.as_deref().unwrap().display().to_string()
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
        let name = format!("{CHUNK_PREFIX}{}", self.rel_path.display());
        lua.load(self.resolved_path.as_deref().unwrap())
            .set_name(name)
            .into_function()
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

/// Install a `require` that always fails, for scripts with no import root.
///
/// A stdin or `-s` script has no directory of its own, and the working
/// directory it happened to be run from is not a boundary anyone chose. Rather
/// than guess one, imports are refused until `-r` names a directory.
pub fn register_denied(lua: &Lua) -> Result<()> {
    let require = lua.create_function(|_, path: String| -> Result<mlua::Value> {
        Err(mlua::Error::runtime(format!(
            "cannot require '{path}': imports are disabled for scripts read from \
             stdin or -s, which have no directory to resolve against; \
             pass -r <dir> to import from that directory"
        )))
    })?;
    lua.globals().raw_set("require", require)?;
    Ok(())
}
