//! Path segments, navigation targets, and errors for the `hcl::edit`
//! cursor API. Ported from the `hrs` crate, adapted to a single-`Body`
//! root (no multi-document `Bodies` target).

use hcl::edit::expr::{Array, Expression, Object, ObjectKey};
use hcl::edit::structure::{Block, Body};
use std::fmt::{Debug, Formatter};

/// A single step in a navigation path built up by traversal calls.
#[derive(Clone)]
pub enum Segment {
    Text {
        text: String,
    },
    Index {
        index: usize,
    },
    Block {
        ident: String,
        labels: Vec<String>,
        nth: Option<usize>,
    },
    Attr {
        key: String,
    },
}

/// Match a block's labels against a label pattern.
///
/// Each pattern element matches one position, except for the wildcards:
/// - a literal string matches that exact label,
/// - `"*"` matches exactly one label (any value),
/// - `"**"` matches zero or more labels (any values).
///
/// An empty pattern matches only a block with **zero** labels. (The "match
/// any labels" default lives at the call sites, which pass `["**"]` when no
/// labels argument is given.) Matching is anchored at both ends: the pattern
/// must consume all labels.
pub fn labels_match(labels: &[&str], pattern: &[String]) -> bool {
    match pattern.split_first() {
        None => labels.is_empty(),
        Some((head, rest)) if head == "**" => {
            // Zero or more labels: try consuming 0, 1, 2, … labels here.
            (0..=labels.len()).any(|skip| labels_match(&labels[skip..], rest))
        }
        Some((head, rest)) => match labels.split_first() {
            None => false,
            Some((label, labels_rest)) => {
                (head == "*" || head == label) && labels_match(labels_rest, rest)
            }
        },
    }
}

impl Segment {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Segment::Text { text } => Some(text),
            Segment::Attr { key } => Some(key),
            Segment::Index { .. } | Segment::Block { .. } => None,
        }
    }

    pub fn as_index(&self) -> Option<usize> {
        match self {
            Segment::Index { index } => Some(*index),
            Segment::Text { .. } | Segment::Block { .. } | Segment::Attr { .. } => None,
        }
    }

    /// Human-readable variant name for `InvalidType` error messages.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Segment::Text { .. } => "key",
            Segment::Index { .. } => "index",
            Segment::Block { .. } => "block",
            Segment::Attr { .. } => "attribute",
        }
    }
}

impl From<&str> for Segment {
    fn from(value: &str) -> Self {
        Segment::Text {
            text: value.to_string(),
        }
    }
}

impl From<usize> for Segment {
    fn from(value: usize) -> Self {
        Segment::Index { index: value }
    }
}

impl Debug for Segment {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Segment::Text { text } => f.write_str(text),
            Segment::Index { index } => write!(f, "[{}]", index),
            Segment::Block { ident, labels, nth } => {
                write!(f, "block({:?}", ident)?;
                if !labels.is_empty() {
                    write!(f, ", {:?}", labels)?;
                }
                if let Some(n) = nth {
                    write!(f, ", {})", n)
                } else {
                    write!(f, ")")
                }
            }
            Segment::Attr { key } => write!(f, "attr({:?})", key),
        }
    }
}

impl std::fmt::Display for Segment {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Segment::Text { text } => f.write_str(text),
            Segment::Attr { key } => write!(f, "attr {key:?}"),
            // 1-based on the way out: the path is spelled as the script did.
            Segment::Index { index } => write!(f, "[{}]", index + 1),
            Segment::Block { ident, labels, nth } => {
                write!(f, "block {ident:?}")?;
                for l in labels {
                    write!(f, " {l:?}")?;
                }
                if let Some(n) = nth {
                    write!(f, " #{}", n + 1)?;
                }
                Ok(())
            }
        }
    }
}

/// Render a path for an error message: `block "a" > attr "x" > [2]`.
pub fn render_path(path: &[Segment]) -> String {
    if path.is_empty() {
        return "the document root".to_string();
    }
    path.iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join(" > ")
}

/// The key of `object` spelled `key`, as written in the source: a bare
/// identifier or a quoted string.
pub fn find_object_key(object: &Object, key: &str) -> Option<ObjectKey> {
    object
        .iter()
        .map(|(k, _)| k)
        .find(|k| object_key_str(k).as_deref() == Some(key))
        .cloned()
}

fn object_key_str(k: &ObjectKey) -> Option<String> {
    match k {
        ObjectKey::Ident(i) => Some(i.as_str().to_string()),
        ObjectKey::Expression(Expression::String(s)) => Some(s.value().to_string()),
        _ => None,
    }
}

/// A new object key for `key`: a bare identifier when it is one, else a
/// quoted string (never a rewritten spelling).
pub fn new_object_key(key: &str) -> ObjectKey {
    match hcl::edit::Ident::try_new(key) {
        Ok(i) => ObjectKey::Ident(i.into()),
        Err(_) => ObjectKey::Expression(Expression::from(key)),
    }
}

/// Wraps an error raised by a script's own callback so `try_*` methods
/// can tell it from a path/structure failure and let it through.
#[derive(Debug)]
pub struct CallbackFailed(pub mlua::Error);

impl std::fmt::Display for CallbackFailed {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for CallbackFailed {}

#[derive(thiserror::Error, Debug)]
pub enum HclError {
    #[error("operation cannot be applied to the document root")]
    NotOnRoot,
    #[error("not found: {segment} (at {})", render_path(path))]
    NotFound {
        /// The path *above* `segment`.
        path: Vec<Segment>,
        segment: Segment,
    },
    /// `index` is 1-based, as the script wrote it.
    #[error("index {index} out of range for an array of length {len}")]
    IndexOutOfBounds { index: i64, len: usize },
    #[error("expected {expected}, but this is a {actual}")]
    InvalidType {
        expected: &'static str,
        actual: &'static str,
    },
    #[error(
        "ambiguous block: {count} blocks match {ident:?} with labels {labels:?}; select one with an index (block(ident, labels, n))"
    )]
    AmbiguousBlock {
        ident: String,
        labels: Vec<String>,
        count: usize,
    },
    #[error(
        "ambiguous key {key:?}: it names both an attribute and a block; use attr() or block() to pick"
    )]
    AmbiguousKey { key: String },
    #[error("a computed `{0}` expression has no Luau value (use raw() to get it verbatim)")]
    UnsupportedHclType(&'static str),
    #[error("a {0} cannot be converted to HCL")]
    UnsupportedLuaType(String),
    #[error("HCL value cannot be converted to a Luau value: {0}")]
    LuaConversion(String),
}

impl From<HclError> for mlua::Error {
    fn from(value: HclError) -> Self {
        mlua::Error::runtime(value.to_string())
    }
}

/// What a resolved path points at, as a mutable borrow into the body.
pub enum Target<'a> {
    Body(&'a mut Body),
    Block(&'a mut Block),
    Object(&'a mut Object),
    Array(&'a mut Array),
    Expr(&'a mut Expression),
}

impl Target<'_> {
    pub fn type_name(&self) -> &'static str {
        match self {
            Target::Body(_) => "body",
            Target::Block(_) => "block",
            Target::Object(_) => "object",
            Target::Array(_) => "array",
            Target::Expr(_) => "expression",
        }
    }
}
