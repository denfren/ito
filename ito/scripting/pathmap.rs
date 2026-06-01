//! Path mapping between the script-visible *virtual* namespace and real
//! filesystem paths, anchored at a single canonical root.
//!
//! Two path vocabularies meet here:
//!
//! - **virtual** — what a script sees: always an absolute namespace
//!   rooted at `/`, so the real file `root/subdir/x.txt` is addressed as
//!   `/subdir/x.txt`. `fs::list`/`fs::glob`/`fs::finder` emit virtual
//!   paths.
//! - **real** — actual filesystem paths, in two output forms.
//!   [`PathMapper::to_real_abs`] turns a virtual (or relative) path into
//!   an **absolute** real path under the root (for disk I/O), and
//!   [`PathMapper::to_real_relative`] turns it into a **root-relative**
//!   string (what `--diff`/`--list-changed` print). The reverse,
//!   [`PathMapper::to_virtual`], turns a real path back into the
//!   `/`-rooted virtual namespace.
//!
//! Every mapping rejects escapes: `..` traversal that climbs above the
//! root is an error, and the result is additionally checked against
//! symlink-based escapes (see [`PathMapper::contained`]).
//!
//! `PathMapper` holds only the canonical root, so it is cheap to clone
//! and `Send + Sync` — handy for handing to consumers (the `j2` include
//! loader, the `import` sandbox) that cannot hold a full `Vfs`.

use std::path::{Component, Path, PathBuf};

use rhai::EvalAltResult;
use thiserror::Error;

/// An error mapping or anchoring a path.
#[derive(Debug, Error)]
pub enum PathError {
    /// The root could not be canonicalized when building the mapper.
    #[error("canonicalize {path}: {source}")]
    Canonicalize {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The canonicalized root is not a directory.
    #[error("not a directory: {0}")]
    NotADirectory(PathBuf),
    /// A virtual/relative path escaped the root (via `..` or a symlink).
    #[error("vfs path escapes root: {0}")]
    Escape(String),
}

/// Mapping errors surface to scripts as Rhai runtime errors.
impl From<PathError> for Box<EvalAltResult> {
    fn from(e: PathError) -> Self {
        e.to_string().into()
    }
}

type Result<T> = std::result::Result<T, PathError>;

/// Maps script-visible paths to/from real paths under a canonical root.
#[derive(Clone)]
pub struct PathMapper {
    /// Canonical root. All resolved paths must stay under it.
    root: PathBuf,
}

impl PathMapper {
    /// Build a mapper rooted at `root`, which must exist (it is
    /// canonicalized so containment checks are reliable).
    pub fn new(root: &Path) -> Result<Self> {
        let canonical = root.canonicalize().map_err(|source| PathError::Canonicalize {
            path: root.to_path_buf(),
            source,
        })?;
        if !canonical.is_dir() {
            return Err(PathError::NotADirectory(canonical));
        }
        Ok(Self { root: canonical })
    }

    /// The canonical root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Lexically reduce a virtual/relative path to its components under the
    /// root, dropping `/`, `.` and resolving `..`. An `..` that pops past
    /// the root is an escape and yields `None`.
    fn lexical(virt: &str) -> Option<PathBuf> {
        let mut rel = PathBuf::new();
        for comp in Path::new(virt).components() {
            match comp {
                Component::RootDir | Component::Prefix(_) | Component::CurDir => {}
                Component::ParentDir => {
                    if !rel.pop() {
                        return None;
                    }
                }
                Component::Normal(seg) => rel.push(seg),
            }
        }
        Some(rel)
    }

    /// Translate a virtual (or relative) script-visible path into an
    /// absolute real path under the root. Absolute (`/`-rooted) and
    /// relative inputs are both taken relative to the root; the intent is
    /// unambiguous. Any `..` traversal that escapes the root is rejected,
    /// and the result is checked against symlink-based escapes (see
    /// [`PathMapper::contained`]).
    pub fn to_real_abs(&self, virt: &str) -> Result<PathBuf> {
        let rel = Self::lexical(virt).ok_or_else(|| PathError::Escape(virt.to_string()))?;
        let real = self.root.join(rel);
        if !self.contained(&real) {
            return Err(PathError::Escape(virt.to_string()));
        }
        Ok(real)
    }

    /// Like [`PathMapper::to_real_abs`], but returns the path relative to
    /// the root (what `--diff`/`--list-changed` print) rather than
    /// absolute. Same escape rejection.
    pub fn to_real_relative(&self, virt: &str) -> Result<PathBuf> {
        let abs = self.to_real_abs(virt)?;
        Ok(abs
            .strip_prefix(&self.root)
            .map(Path::to_path_buf)
            .unwrap_or(abs))
    }

    /// Map an already-real path to a string relative to the root, for
    /// display. The strip is purely lexical, so it holds whether or not the
    /// file exists on disk. Falls back to the absolute path on the off
    /// chance the path is not under the root.
    pub fn relativize(&self, real: &Path) -> String {
        real.strip_prefix(&self.root)
            .map(|rel| rel.to_string_lossy().into_owned())
            .unwrap_or_else(|_| real.to_string_lossy().into_owned())
    }

    /// Map a real path back into the script-visible absolute virtual
    /// namespace (`/subdir/x.txt`; the root itself is `/`). Falls back to
    /// the absolute path if it is not under the root.
    pub fn to_virtual(&self, real: &Path) -> String {
        match real.strip_prefix(&self.root) {
            Ok(rest) => {
                let s = rest.to_string_lossy();
                if s.is_empty() {
                    "/".to_string()
                } else {
                    format!("/{s}")
                }
            }
            Err(_) => real.to_string_lossy().into_owned(),
        }
    }

    /// Verify that `real` (any real path) does not escape the canonical
    /// root, accounting for symlinks.
    ///
    /// `real` may point at a file that does not exist yet, which cannot be
    /// canonicalized. We canonicalize the longest existing prefix: strip
    /// trailing segments until canonicalization succeeds, then check that
    /// the resolved ancestor is still inside the root. The stripped
    /// (nonexistent) suffix cannot itself be a symlink, so the deepest
    /// existing ancestor is the only thing that matters.
    pub fn contained(&self, real: &Path) -> bool {
        let mut probe = real;
        loop {
            if let Ok(canonical) = probe.canonicalize() {
                return canonical.starts_with(&self.root);
            }
            match probe.parent() {
                Some(parent) => probe = parent,
                // Walked above the filesystem root without canonicalizing.
                None => return false,
            }
        }
    }
}
