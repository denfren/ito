//! Module resolver for `import` statements in Rhai scripts.
//!
//! Rhai's stock `FileModuleResolver` overwrites the loaded module's AST
//! source with the *import string* (e.g. `"ansible/lint-key-order"`),
//! which means nested imports inside that module resolve against the
//! import string's parent (`"ansible"` relative to CWD) instead of the
//! module's actual file directory. That breaks grouper scripts that
//! re-export sibling modules.
//!
//! This resolver:
//!
//! 1. Requires a `root` directory (the absolute, canonical path of the
//!    initial script's parent). Without one, no resolver is installed
//!    by the caller and `import` becomes an error — `import` is only
//!    available when the engine has a known on-disk root to anchor to.
//! 2. Resolves a relative import against the loading script's
//!    directory, mirroring `FileModuleResolver`'s documented behavior,
//!    by stamping the resolved canonical file path as the loaded AST's
//!    source.
//! 3. Canonicalizes every resolved path and rejects anything that
//!    escapes `root`, matching the CLI's "path is outside the working
//!    directory" check for input files.
//! 4. Caches each module by its canonical file path so a file is
//!    compiled and evaluated at most once per engine.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use rhai::{AST, Engine, EvalAltResult, Module, ModuleResolver, Position, Scope, Shared};

const SCRIPT_EXTENSION: &str = "rhai";

type RhaiResult<T> = Result<T, Box<EvalAltResult>>;

pub struct ScriptModuleResolver {
    /// Canonical absolute directory that all imports must resolve
    /// inside. The initial script's parent.
    root: PathBuf,
    /// Modules already evaluated, keyed by canonical file path.
    cache: RefCell<Vec<(PathBuf, Shared<Module>)>>,
}

impl ScriptModuleResolver {
    /// Build a resolver rooted at `root` (must already be absolute and
    /// canonical — typically `script_path.canonicalize()?.parent()`).
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            cache: RefCell::new(Vec::new()),
        }
    }

    fn err_module(path: &str, pos: Position) -> Box<EvalAltResult> {
        Box::new(EvalAltResult::ErrorModuleNotFound(path.to_string(), pos))
    }

    /// Compute the candidate file path for `path` relative to `source`,
    /// then canonicalize and verify it lives under `root`.
    fn resolved_path(
        &self,
        source: Option<&str>,
        path: &str,
        pos: Position,
    ) -> RhaiResult<PathBuf> {
        let raw = Path::new(path);
        let mut candidate = if raw.is_relative() {
            source
                .and_then(|s| Path::new(s).parent().map(Path::to_path_buf))
                .unwrap_or_else(|| self.root.clone())
        } else {
            PathBuf::new()
        };
        candidate.push(raw);
        candidate.set_extension(SCRIPT_EXTENSION);

        let canonical = candidate
            .canonicalize()
            .map_err(|_| Self::err_module(path, pos))?;
        if !canonical.starts_with(&self.root) {
            return Err(Self::err_module(path, pos));
        }
        Ok(canonical)
    }

    fn cached(&self, file_path: &Path) -> Option<Shared<Module>> {
        self.cache
            .borrow()
            .iter()
            .find(|(p, _)| p == file_path)
            .map(|(_, m)| m.clone())
    }

    fn store(&self, file_path: PathBuf, module: Shared<Module>) {
        self.cache.borrow_mut().push((file_path, module));
    }

    fn compile(engine: &Engine, file_path: &Path, pos: Position) -> RhaiResult<AST> {
        let mut ast = engine
            .compile_file(file_path.to_path_buf())
            .map_err(|err| match *err {
                EvalAltResult::ErrorSystem(.., ref e) if e.is::<std::io::Error>() => Box::new(
                    EvalAltResult::ErrorModuleNotFound(file_path.display().to_string(), pos),
                ),
                _ => Box::new(EvalAltResult::ErrorInModule(
                    file_path.display().to_string(),
                    err,
                    pos,
                )),
            })?;
        // Stamp the AST source with the resolved canonical file path so
        // nested imports anchor against this module's directory, not
        // the bare import string Rhai's stock resolver would set.
        ast.set_source(file_path.to_string_lossy().as_ref());
        Ok(ast)
    }
}

impl ModuleResolver for ScriptModuleResolver {
    fn resolve(
        &self,
        engine: &Engine,
        source: Option<&str>,
        path: &str,
        pos: Position,
    ) -> RhaiResult<Shared<Module>> {
        let file_path = self.resolved_path(source, path, pos)?;
        if let Some(m) = self.cached(&file_path) {
            return Ok(m);
        }
        let ast = Self::compile(engine, &file_path, pos)?;
        let mut module_scope = Scope::new();
        module_scope.push_constant("MAIN", false);
        let module: Shared<Module> = Module::eval_ast_as_new(module_scope, &ast, engine)
            .map_err(|err| {
                Box::new(EvalAltResult::ErrorInModule(
                    file_path.display().to_string(),
                    err,
                    pos,
                ))
            })?
            .into();
        self.store(file_path, module.clone());
        Ok(module)
    }

    fn resolve_ast(
        &self,
        engine: &Engine,
        source: Option<&str>,
        path: &str,
        pos: Position,
    ) -> Option<Result<AST, Box<EvalAltResult>>> {
        let file_path = match self.resolved_path(source, path, pos) {
            Ok(p) => p,
            Err(e) => return Some(Err(e)),
        };
        Some(Self::compile(engine, &file_path, pos))
    }
}
