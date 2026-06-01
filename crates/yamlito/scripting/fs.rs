//! Per-invocation `input` handle exposed to scripts under `ito run`.
//!
//! Each positional CLI argument is dispatched to the script as a single
//! invocation with `input` bound to either a `FileInput` (file argument
//! or stdin) or a `DirInput` (directory argument). Both Rhai types
//! expose the same surface: `input.glob(pattern)` / `input.files()`
//! returning `[YamlFile]`, plus path-component properties (`path`,
//! `basename`, `dirname`, `stem`, `extension`).
//!
//! `DirInput.glob` walks the directory's subtree, matches each file
//! against the pattern (globset, supports brace expansion), and returns
//! `YamlFile`s. `DirInput.files()` returns every file under the root.
//! `FileInput.glob` returns `[input_as_yaml_file]` if the pattern
//! matches the file's path, otherwise `[]` — the same script body works
//! in both modes. `FileInput.files()` always returns `[self]`.
//!
//! Symlinks under a `DirInput` are not followed; the directory itself
//! is the access fence.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use globset::{Glob, GlobBuilder, GlobMatcher};
use rhai::plugin::*;
use rhai::{Dynamic, EvalAltResult, ImmutableString};

use crate::scripting::cursor::{
    YamlFile, make_yaml_file, make_yaml_file_lazy, make_yaml_file_with_real_path,
};

/// Records every `YamlFile` materialized via `input.glob(...)` so the
/// runner can iterate them at script exit (for `--check`, write-back,
/// and multi-writer detection).
pub type ParsedRegistry = Rc<RefCell<Vec<YamlFile>>>;

/// Path that stands in for stdin in user-visible output. Distinct from
/// any real on-disk path so nothing is ever written there.
pub const STDIN_PATH: &str = "/dev/stdin";

/// File-shaped invocation: stdin or a single file argument. The
/// `path` is the file's display path (`/dev/stdin` for stdin, the
/// canonical absolute disk path otherwise). When `real_path` is
/// `Some(...)` and the file ends up dirty, the runner writes back
/// there; when `None` (stdin), the runner emits to stdout.
#[derive(Clone)]
pub struct FileInput {
    path: String,
    source: String,
    real_path: Option<PathBuf>,
    /// Materialized once on first `glob()` call so repeated globs see
    /// the same `YamlFile` instance and accumulated mutations.
    cached: Rc<RefCell<Option<YamlFile>>>,
    registry: ParsedRegistry,
}

/// Directory-shaped invocation. `glob` walks `root` recursively, parses
/// each match eagerly, and produces `YamlFile`s that share the registry
/// for write-back at script exit.
#[derive(Clone)]
pub struct DirInput {
    root: PathBuf,
    /// Snapshot of every regular file under `root`, populated lazily on
    /// the first `glob()` / `list_files()` call.
    listing: Rc<RefCell<Option<Vec<PathBuf>>>>,
    /// Cache of `(real_path -> YamlFile)` so two `glob` calls with
    /// overlapping patterns return the same handle (and mutations on
    /// one are visible through the other).
    parsed_cache: Rc<RefCell<std::collections::HashMap<PathBuf, YamlFile>>>,
    registry: ParsedRegistry,
}

impl FileInput {
    fn as_yaml_file(&self) -> YamlFile {
        let mut guard = self.cached.borrow_mut();
        if let Some(existing) = guard.as_ref() {
            return existing.clone();
        }
        let file = match &self.real_path {
            Some(real) => {
                make_yaml_file_with_real_path(self.path.clone(), self.source.clone(), real.clone())
            }
            None => make_yaml_file(self.path.clone(), self.source.clone()),
        };
        self.registry.borrow_mut().push(file.clone());
        *guard = Some(file.clone());
        file
    }
}

#[export_module]
#[allow(non_snake_case)]
pub mod file_input_api {
    use super::*;

    #[rhai_fn(get = "path", name = "path", pure)]
    pub fn path(f: &mut FileInput) -> String {
        f.path.clone()
    }

    #[rhai_fn(get = "basename", name = "basename", pure)]
    pub fn basename(f: &mut FileInput) -> String {
        path_basename(Path::new(&f.path))
    }

    #[rhai_fn(get = "dirname", name = "dirname", pure)]
    pub fn dirname(f: &mut FileInput) -> String {
        path_dirname(Path::new(&f.path))
    }

    #[rhai_fn(get = "stem", name = "stem", pure)]
    pub fn stem(f: &mut FileInput) -> String {
        path_stem(Path::new(&f.path))
    }

    #[rhai_fn(get = "extension", name = "extension", pure)]
    pub fn extension(f: &mut FileInput) -> String {
        path_extension(Path::new(&f.path))
    }

    /// Match `pattern` against this file's path. Returns a one-element
    /// array with the parsed `YamlFile` on a hit, or `[]` on a miss.
    /// Lets the same script body — `for f in input.glob("**/*.{yml,yaml}") { ... }`
    /// — work for both file and directory invocations.
    #[rhai_fn(name = "glob", return_raw, pure)]
    pub fn glob(
        f: &mut FileInput,
        pattern: ImmutableString,
    ) -> Result<rhai::Array, Box<EvalAltResult>> {
        let matcher = build_matcher(&pattern)?;
        if !matcher.is_match(Path::new(&f.path)) {
            return Ok(rhai::Array::new());
        }
        Ok(vec![Dynamic::from(f.as_yaml_file())])
    }

    /// Return this file as a one-element array. Mirrors `DirInput.files()`
    /// so the same script works for both file and directory invocations.
    #[rhai_fn(name = "files", return_raw, pure)]
    pub fn files(f: &mut FileInput) -> Result<rhai::Array, Box<EvalAltResult>> {
        Ok(vec![Dynamic::from(f.as_yaml_file())])
    }

    /// The underlying `YamlFile` for this input. Shorthand for
    /// `input.files()[0]` when the script knows it has a file input.
    #[rhai_fn(get = "file", name = "file", pure)]
    pub fn file(f: &mut FileInput) -> YamlFile {
        f.as_yaml_file()
    }
}

impl DirInput {
    fn ensure_listing(&self) -> std::cell::Ref<'_, Vec<PathBuf>> {
        {
            let mut guard = self.listing.borrow_mut();
            if guard.is_none() {
                let mut out: Vec<PathBuf> = Vec::new();
                walk(&self.root, &self.root, &mut out);
                out.sort();
                *guard = Some(out);
            }
        }
        std::cell::Ref::map(self.listing.borrow(), |o| o.as_ref().unwrap())
    }

    fn parse_or_get(&self, real: &Path) -> YamlFile {
        let mut cache = self.parsed_cache.borrow_mut();
        if let Some(existing) = cache.get(real) {
            return existing.clone();
        }
        // Lazy: don't read or parse until the script actually touches
        // the document. That way `for f in input.glob(...)` can filter
        // out files (by stem, basename, etc.) without paying for I/O
        // or aborting on unparseable but irrelevant files.
        let file = make_yaml_file_lazy(real.to_string_lossy().to_string(), real.to_path_buf());
        cache.insert(real.to_path_buf(), file.clone());
        self.registry.borrow_mut().push(file.clone());
        file
    }
}

#[export_module]
#[allow(non_snake_case)]
pub mod dir_input_api {
    use super::*;

    #[rhai_fn(get = "path", name = "path", pure)]
    pub fn path(d: &mut DirInput) -> String {
        d.root.to_string_lossy().to_string()
    }

    #[rhai_fn(get = "basename", name = "basename", pure)]
    pub fn basename(d: &mut DirInput) -> String {
        path_basename(&d.root)
    }

    #[rhai_fn(get = "dirname", name = "dirname", pure)]
    pub fn dirname(d: &mut DirInput) -> String {
        path_dirname(&d.root)
    }

    #[rhai_fn(get = "stem", name = "stem", pure)]
    pub fn stem(d: &mut DirInput) -> String {
        path_stem(&d.root)
    }

    #[rhai_fn(get = "extension", name = "extension", pure)]
    pub fn extension(d: &mut DirInput) -> String {
        path_extension(&d.root)
    }

    /// Match `pattern` against every regular file under this directory.
    /// Supports brace expansion (`*.{yml,yaml}`) and all globset syntax.
    #[rhai_fn(name = "glob", return_raw, pure)]
    pub fn glob(
        d: &mut DirInput,
        pattern: ImmutableString,
    ) -> Result<rhai::Array, Box<EvalAltResult>> {
        let matcher = if Path::new(pattern.as_str()).is_absolute() {
            // Path-aware containment check.
            if !Path::new(pattern.as_str()).starts_with(&d.root) {
                return Err(format!(
                    "glob pattern {pattern} is outside the input directory {}",
                    d.root.display()
                )
                .into());
            }
            build_matcher(&pattern)?
        } else {
            let root_str = d.root.to_string_lossy();
            build_matcher(&format!("{root_str}/**/{pattern}"))?
        };

        let listing = d.ensure_listing();
        let mut out: Vec<Dynamic> = Vec::new();
        for path in listing.iter() {
            if !matcher.is_match(path) {
                continue;
            }
            let parsed = d.parse_or_get(path);
            out.push(Dynamic::from(parsed));
        }
        Ok(out)
    }

    /// Return every regular file under this directory as `[YamlFile]`.
    /// Use Rhai's array methods to filter down to what you need.
    #[rhai_fn(name = "files", return_raw, pure)]
    pub fn files(d: &mut DirInput) -> Result<rhai::Array, Box<EvalAltResult>> {
        let listing = d.ensure_listing();
        let out: Vec<Dynamic> = listing
            .iter()
            .map(|path| Dynamic::from(d.parse_or_get(path)))
            .collect();
        Ok(out)
    }
}

fn path_basename(p: &Path) -> String {
    p.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn path_dirname(p: &Path) -> String {
    p.parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn path_stem(p: &Path) -> String {
    p.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn path_extension(p: &Path) -> String {
    p.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn build_matcher(pattern: &str) -> Result<GlobMatcher, Box<EvalAltResult>> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|g: Glob| g.compile_matcher())
        .map_err(|e| format!("invalid glob pattern: {e}").into())
}

/// Walk `dir` recursively, collecting every regular file into `out`.
/// Symlinks are skipped so containment under `root` survives.
fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.file_type() else {
            continue;
        };
        if meta.is_symlink() {
            continue;
        }
        if meta.is_dir() {
            walk(root, &path, out);
        } else if meta.is_file()
            && let Ok(canonical) = path.canonicalize()
            && canonical.starts_with(root)
        {
            out.push(canonical);
        }
    }
}

/// Materialize (or fetch the cached) `YamlFile` backing a `FileInput`.
/// The runner uses this to bind file-only globals (`d`, `s`, etc.)
/// without going through the script-side `input.glob(...)` path.
pub fn file_input_as_yaml_file(input: &FileInput) -> YamlFile {
    input.as_yaml_file()
}

/// Builder for the per-invocation input handles. The runner constructs
/// one of these per positional CLI arg, then binds the result on
/// `scope` as `input`.
pub struct InputBuilder {
    registry: ParsedRegistry,
}

impl InputBuilder {
    pub fn new(registry: ParsedRegistry) -> Self {
        Self { registry }
    }

    /// Build a stdin-shaped `FileInput`. `source` is the bytes already
    /// read from stdin. The handle has no `real_path`, so write-back
    /// emits to stdout instead.
    pub fn stdin(&self, source: String) -> FileInput {
        FileInput {
            path: STDIN_PATH.to_string(),
            source,
            real_path: None,
            cached: Rc::new(RefCell::new(None)),
            registry: Rc::clone(&self.registry),
        }
    }

    /// Build a `FileInput` from an on-disk file. `path` is canonicalized
    /// before use; the original argument string is irrelevant once we've
    /// settled on a canonical disk path.
    pub fn file(&self, path: &Path) -> Result<FileInput, String> {
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("canonicalize {}: {e}", path.display()))?;
        let source = std::fs::read_to_string(&canonical)
            .map_err(|e| format!("read error {}: {e}", canonical.display()))?;
        Ok(FileInput {
            path: canonical.to_string_lossy().to_string(),
            source,
            real_path: Some(canonical),
            cached: Rc::new(RefCell::new(None)),
            registry: Rc::clone(&self.registry),
        })
    }

    /// Build a `DirInput` from an on-disk directory. The directory is
    /// canonicalized before use.
    pub fn dir(&self, path: &Path) -> Result<DirInput, String> {
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("canonicalize {}: {e}", path.display()))?;
        if !canonical.is_dir() {
            return Err(format!("not a directory: {}", canonical.display()));
        }
        Ok(DirInput {
            root: canonical,
            listing: Rc::new(RefCell::new(None)),
            parsed_cache: Rc::new(RefCell::new(std::collections::HashMap::new())),
            registry: Rc::clone(&self.registry),
        })
    }
}

/// Register the `FileInput` and `DirInput` types and their plugin
/// modules with `engine`. Returns a fresh `ParsedRegistry` shared by
/// every input handle the runner builds with the matching
/// `InputBuilder`.
pub fn register(engine: &mut rhai::Engine) -> ParsedRegistry {
    engine.register_type_with_name::<FileInput>("FileInput");
    engine.register_type_with_name::<DirInput>("DirInput");
    engine.register_global_module(exported_module!(file_input_api).into());
    engine.register_global_module(exported_module!(dir_input_api).into());

    Rc::new(RefCell::new(Vec::new()))
}
