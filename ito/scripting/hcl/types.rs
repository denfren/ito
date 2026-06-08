//! Path segments, navigation targets, and errors for the `hcl::edit`
//! cursor API. Ported from the `hrs` crate, adapted to a single-`Body`
//! root (no multi-document `Bodies` target).

use hcl::edit::expr::{Array, Expression, Object};
use hcl::edit::structure::{Block, Body};
use rhai::{EvalAltResult, Position};
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
            Segment::Text { .. } => "Text segment",
            Segment::Index { .. } => "Index segment",
            Segment::Block { .. } => "Block segment",
            Segment::Attr { .. } => "Attr segment",
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

#[derive(thiserror::Error, Debug)]
pub enum HclError {
    #[error("Operation can not be applied to root")]
    NotOnRoot,
    #[error("Segment {segment:?} not found in path")]
    NotFound {
        path: Vec<Segment>,
        segment: Segment,
    },
    #[error("Index {index} out of bounds for array of length {len}")]
    IndexOutOfBounds { index: i64, len: usize },
    #[error("Invalid type: expected {expected}, got {actual}")]
    InvalidType {
        expected: &'static str,
        actual: &'static str,
    },
    #[error(
        "Ambiguous block: found {count} blocks matching '{ident}' with labels {labels:?}, use nth index to select"
    )]
    AmbiguousBlock {
        ident: String,
        labels: Vec<String>,
        count: usize,
    },
    #[error(
        "Ambiguous key '{key}': matches both attribute and block, use attr() or block() to disambiguate"
    )]
    AmbiguousKey { key: String },
    #[error("HCL expression type '{0}' cannot be converted to Rhai")]
    UnsupportedHclType(&'static str),
    #[error("Rhai type '{0}' cannot be converted to HCL")]
    UnsupportedRhaiType(String),
}

impl From<HclError> for Box<EvalAltResult> {
    fn from(value: HclError) -> Self {
        Box::new(EvalAltResult::ErrorRuntime(
            value.to_string().into(),
            Position::NONE,
        ))
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
            Target::Body(_) => "Body",
            Target::Block(_) => "Block",
            Target::Object(_) => "Object",
            Target::Array(_) => "Array",
            Target::Expr(_) => "Expr",
        }
    }
}
