//! Module resolution for `import` statements.
//!
//! Rhai's default resolver loads `import` paths relative to the process
//! working directory and can escape anywhere on disk. `ito` instead
//! resolves imports relative to the *importing file's own directory* (the
//! CWD / VFS root for the entry `exec`/stdin script, and — for nested
//! imports — the directory of whichever module ran the `import`), and,
//! when the entry script lives inside the `-C` working directory (the VFS
//! root), confines *every* resolved hop to that root using the same
//! containment check the `fs` module uses ([`PathMapper::contained`]).
//!
//! ## How the base directory is chosen per hop
//!
//! Rhai calls [`ModuleResolver::resolve`] with `source` set to the source
//! string of the *importing* compilation unit. We always hand the inner
//! [`FileModuleResolver`] an **absolute** path and set each loaded
//! module's source to that absolute path, so:
//!
//! - top-level imports (the entry script — `source` is `None`) resolve
//!   against `entry_base`;
//! - nested imports resolve against the absolute directory of the module
//!   that issued them (`source`'s parent), so a module in a subdirectory
//!   imports *its own* siblings, not the entry script's.
//!
//! ## Sandboxing
//!
//! When `sandbox` is `Some`, the absolute target of *every* hop is checked
//! for containment under the VFS root; an escape (via `..` or a symlink),
//! at any depth, is rejected. A script that lives outside the root is not
//! sandboxed (`sandbox` is `None`) and resolves with ordinary file lookup.

use std::path::{Path, PathBuf};

use rhai::module_resolvers::FileModuleResolver;
use rhai::{Engine, EvalAltResult, Module, ModuleResolver, Position, Scope, Shared};

use ito::scripting::PathMapper;

/// The script file extension `import` paths get, matching Rhai's default.
const SCRIPT_EXT: &str = "rhai";

/// A module resolver that resolves `import` paths relative to the
/// importing file (the entry base for the top-level script) and
/// optionally confines every hop to a VFS root.
pub(crate) struct ImportResolver {
    /// Base directory for top-level imports (the entry script's directory,
    /// or the VFS root for inline/stdin). Absolute.
    entry_base: PathBuf,
    /// Delegate doing the actual compilation + caching. It carries *no*
    /// base path, so we feed it absolute paths and it loads them directly.
    inner: FileModuleResolver,
    /// When `Some`, every resolved target must stay under the VFS root.
    sandbox: Option<PathMapper>,
}

impl ImportResolver {
    /// Build a resolver for a script.
    ///
    /// `entry_base` is where the entry script's relative imports resolve
    /// from (already canonical). `root` is the canonical `-C` root. The
    /// resolver is sandboxed iff `entry_base` is contained under the root
    /// (inline/stdin pass `entry_base == root`, which is contained, so they
    /// are sandboxed too).
    ///
    /// `module_globals` are constants seeded into the scope each module is
    /// compiled with, so the host globals (`ITO_VERSION`, and `MAIN` —
    /// always `false` in a module, since a module is never the entry point)
    /// are constant-folded into the module's bodies and visible inside its
    /// functions, not just the entry script's top-level scope.
    pub(crate) fn new(
        entry_base: &Path,
        root: &PathMapper,
        module_globals: Scope<'static>,
    ) -> Self {
        let sandbox = root.contained(entry_base).then(|| root.clone());
        let mut inner = FileModuleResolver::new();
        inner.set_scope(module_globals);
        Self {
            entry_base: entry_base.to_path_buf(),
            inner,
            sandbox,
        }
    }

    /// Compute the absolute target file for an `import path` issued from
    /// `source` (the importing module's source string, if any). Relative
    /// `path`s resolve against the importing file's directory (or
    /// `entry_base` for the top-level script); the `.rhai` extension is
    /// forced, matching Rhai's [`FileModuleResolver`].
    fn target(&self, source: Option<&str>, path: &str) -> PathBuf {
        let raw = Path::new(path);
        let mut file = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            let base = source
                .map(Path::new)
                .and_then(Path::parent)
                .filter(|p| !p.as_os_str().is_empty())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.entry_base.clone());
            base.join(raw)
        };
        file.set_extension(SCRIPT_EXT);
        file
    }
}

impl ModuleResolver for ImportResolver {
    fn resolve(
        &self,
        engine: &Engine,
        source: Option<&str>,
        path: &str,
        pos: Position,
    ) -> Result<Shared<Module>, Box<EvalAltResult>> {
        let target = self.target(source, path);

        if let Some(sandbox) = &self.sandbox
            && !sandbox.contained(&target)
        {
            return Err(Box::new(EvalAltResult::ErrorModuleNotFound(
                format!("{path} (import escapes the working directory root)"),
                pos,
            )));
        }

        // Hand the delegate an absolute path: it loads it directly,
        // ignoring its (empty) base, and sets the loaded module's source to
        // this absolute path — so a nested import from that module resolves
        // against *its* directory, not the entry script's.
        let abs = target.to_string_lossy();
        self.inner.resolve(engine, source, &abs, pos)
    }
}
