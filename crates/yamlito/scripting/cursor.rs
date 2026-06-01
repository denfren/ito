//! Typed cursors for the Rhai scripting surface.
//!
//! Each cursor is a separate Rust type registered with Rhai. That gives
//! us shift-left error reporting: calling `.read()` on a mapping surfaces
//! as a Rhai "method not found for MappingCursor" at dispatch, before our
//! handler code runs.
//!
//! Navigation (`.foo`, `[key]`, `[i]`) resolves the step against the live
//! tree and fails immediately with a clear error if the path doesn't
//! exist. There is no `Missing` cursor or lazy-chain.
//!
//! All four cursors share a `Rc<RefCell<SyntaxTree>>` so mutations from
//! one are visible to the others.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use rhai::plugin::*;
use rhai::{Dynamic, EvalAltResult, ImmutableString};
use rowan::NodeOrToken;

use crate::SyntaxTree;
use crate::ast::{AstNode, Mapping, Node, Scalar, Value};
use crate::edit::{TreeEdit, apply_edit, mk_node, mk_token};
use crate::lexer::{as_plain_scalar, as_single_quoted};
use crate::scripting::comment;
use crate::syntax::{SyntaxKind, SyntaxNode};

/// Shared handle to the parsed tree. Cloning is cheap.
type TreeHandle = Rc<RefCell<SyntaxTree>>;

/// Null object returned for an empty document. All navigation methods
/// return empty collections.
#[derive(Clone)]
pub struct NullCursor;

/// Points at a block or flow mapping.
#[derive(Clone)]
pub struct MappingCursor {
    tree: TreeHandle,
    node: SyntaxNode,
}

/// Points at a block or flow sequence.
#[derive(Clone)]
pub struct SequenceCursor {
    tree: TreeHandle,
    node: SyntaxNode,
}

/// Points at a scalar (any style).
#[derive(Clone)]
pub struct ScalarCursor {
    tree: TreeHandle,
    node: SyntaxNode,
}

/// Points at the whole stream. Primary surface for multi-document
/// streams; indexable by document number.
#[derive(Clone)]
pub struct StreamCursor {
    tree: TreeHandle,
    node: SyntaxNode,
}

/// Yielded by mapping iteration. Holds the decoded key and a value
/// cursor. Exposed to scripts as `YamlEntry` with `.key()` and
/// `.value()` accessors.
#[derive(Clone)]
pub struct MapEntry {
    key: String,
    value: Dynamic,
    key_node: SyntaxNode,
    tree: TreeHandle,
}

impl MapEntry {
    /// Byte span of the key node.
    pub fn key_span(&self) -> Range<usize> {
        let r = self.key_node.text_range();
        usize::from(r.start())..usize::from(r.end())
    }

    /// Byte span of the value node, if any.
    pub fn value_span(&self) -> Option<Range<usize>> {
        span_of_dynamic(&self.value)
    }

    /// Live source text of the underlying tree.
    pub fn tree_source(&self) -> String {
        self.tree.borrow().emit()
    }

    /// Cursor (scalar) at the entry's key node, so scripts can pass
    /// it to `file.lint(...)` for a path-annotated render of the key.
    pub fn key_cursor(&self) -> Dynamic {
        cursor_for(Rc::clone(&self.tree), self.key_node.clone())
    }
}

macro_rules! cursor_node_accessors {
    ($t:ty) => {
        impl $t {
            /// Byte span of the cursor's node.
            pub fn node_span(&self) -> Range<usize> {
                let r = self.node.text_range();
                usize::from(r.start())..usize::from(r.end())
            }

            /// Live source text of the underlying tree.
            pub fn tree_source(&self) -> String {
                self.tree.borrow().emit()
            }
        }
    };
}

cursor_node_accessors!(MappingCursor);
cursor_node_accessors!(SequenceCursor);
cursor_node_accessors!(ScalarCursor);
cursor_node_accessors!(StreamCursor);

impl MappingCursor {
    /// Byte span of the key scalar whose decoded text equals `key`.
    pub fn key_token_span(&self, key: &str) -> Option<Range<usize>> {
        let key_node = mapping_key_node(&self.node, key)?;
        let r = key_node.text_range();
        Some(usize::from(r.start())..usize::from(r.end()))
    }

    /// Byte span of the value node for `key`.
    pub fn value_node_span(&self, key: &str) -> Option<Range<usize>> {
        let n = resolve_map_value(&self.node, key)?;
        let r = n.text_range();
        Some(usize::from(r.start())..usize::from(r.end()))
    }
}

fn mapping_key_node(map_node: &SyntaxNode, key: &str) -> Option<SyntaxNode> {
    let node = Node::cast(map_node.clone())?;
    for (k, _) in node.as_mapping()?.pairs() {
        if let Node::Scalar(s) = &k
            && s.decoded().ok().as_deref() == Some(key)
        {
            return Some(s.syntax().clone());
        }
    }
    None
}

/// Build the root cursor for a tree, dispatching on the root document's
/// top-level node kind. Returns `()` for an empty document so that
/// stream-level scripts (`s.fmt()`, `s.fix()`) still run; any attempt to
/// navigate into `d`/`doc` will then fail with a clear Rhai dispatch
/// error against the unit value.
pub fn root_cursor(tree: TreeHandle) -> Result<Dynamic, Box<EvalAltResult>> {
    let root = tree.borrow().root().clone();
    Ok(crate::ast::Stream::cast(root)
        .and_then(|stream| stream.documents().next())
        .and_then(|doc| doc.root_node())
        .map(|n| cursor_for(tree, node_syntax(&n)))
        .unwrap_or_else(|| Dynamic::from(NullCursor)))
}

/// Wrap a `Node`-resolved SyntaxNode into the right cursor variant and
/// box it as a Dynamic.
fn cursor_for(tree: TreeHandle, node: SyntaxNode) -> Dynamic {
    match node.kind() {
        SyntaxKind::BLOCK_MAPPING | SyntaxKind::FLOW_MAPPING => {
            Dynamic::from(MappingCursor { tree, node })
        }
        SyntaxKind::BLOCK_SEQUENCE | SyntaxKind::FLOW_SEQUENCE => {
            Dynamic::from(SequenceCursor { tree, node })
        }
        SyntaxKind::SCALAR => Dynamic::from(ScalarCursor { tree, node }),
        // ALIAS: expose as a scalar cursor for now — read() will emit
        // the alias text. Revisit if users want to follow aliases.
        SyntaxKind::ALIAS => Dynamic::from(ScalarCursor { tree, node }),
        other => Dynamic::from(format!("unexpected node kind: {other:?}")),
    }
}

/// Look up a mapping entry by key and produce a cursor on its value.
fn map_step_key(m: &MappingCursor, key: &str) -> Result<Dynamic, Box<EvalAltResult>> {
    let value_node =
        resolve_map_value(&m.node, key).ok_or_else(|| format!("key not found: {key}"))?;
    Ok(cursor_for(Rc::clone(&m.tree), value_node))
}

/// Look up a sequence element by index.
fn seq_step_index(s: &SequenceCursor, i: i64) -> Result<Dynamic, Box<EvalAltResult>> {
    let idx: usize = if i < 0 {
        return Err(format!("negative index: {i}").into());
    } else {
        i as usize
    };
    let elem =
        resolve_seq_index(&s.node, idx).ok_or_else(|| format!("index out of range: {idx}"))?;
    Ok(cursor_for(Rc::clone(&s.tree), elem))
}

/// Resolve a mapping key against a SyntaxNode that is a BLOCK or FLOW
/// mapping. Returns the value's SyntaxNode.
fn resolve_map_value(map_node: &SyntaxNode, key: &str) -> Option<SyntaxNode> {
    // Use the AST view to decode keys.
    let node = Node::cast(map_node.clone())?;
    let looked_up = node.as_mapping()?.get(key)?;
    Some(node_syntax(&looked_up))
}

fn resolve_seq_index(seq_node: &SyntaxNode, i: usize) -> Option<SyntaxNode> {
    let node = Node::cast(seq_node.clone())?;
    let looked_up = node.as_sequence()?.iter().nth(i)?;
    Some(node_syntax(&looked_up))
}

fn node_syntax(n: &Node) -> SyntaxNode {
    n.syntax().clone()
}

//
// `register_iterator::<T>()` needs T: IntoIterator. We snapshot the
// children up front at iterator creation so the iteration shape is
// fixed; subsequent scalar writes on yielded cursors stay visible (they
// share the tree handle) but structural edits during iteration do not
// shift what remains to yield.

pub struct CursorIter {
    items: std::vec::IntoIter<Dynamic>,
}

impl Iterator for CursorIter {
    type Item = Dynamic;
    fn next(&mut self) -> Option<Self::Item> {
        self.items.next()
    }
}

impl IntoIterator for SequenceCursor {
    type Item = Dynamic;
    type IntoIter = CursorIter;
    fn into_iter(self) -> Self::IntoIter {
        let children: Vec<SyntaxNode> = Node::cast(self.node.clone())
            .as_ref()
            .and_then(Node::as_sequence)
            .map(|s| s.iter().map(|n| node_syntax(&n)).collect())
            .unwrap_or_default();
        let items: Vec<Dynamic> = children
            .into_iter()
            .map(|n| cursor_for(Rc::clone(&self.tree), n))
            .collect();
        CursorIter {
            items: items.into_iter(),
        }
    }
}

pub struct MapIter {
    items: std::vec::IntoIter<MapEntry>,
}

impl Iterator for MapIter {
    type Item = MapEntry;
    fn next(&mut self) -> Option<Self::Item> {
        self.items.next()
    }
}

impl IntoIterator for MappingCursor {
    type Item = MapEntry;
    type IntoIter = MapIter;
    fn into_iter(self) -> Self::IntoIter {
        let pairs: Vec<(String, SyntaxNode, SyntaxNode)> = Node::cast(self.node.clone())
            .as_ref()
            .and_then(Node::as_mapping)
            .map(collect_map_pairs)
            .unwrap_or_default();
        let items: Vec<MapEntry> = pairs
            .into_iter()
            .map(|(k, kn, v)| MapEntry {
                key: k,
                value: cursor_for(Rc::clone(&self.tree), v),
                key_node: kn,
                tree: Rc::clone(&self.tree),
            })
            .collect();
        MapIter {
            items: items.into_iter(),
        }
    }
}

fn collect_map_pairs(m: &dyn Mapping) -> Vec<(String, SyntaxNode, SyntaxNode)> {
    m.pairs()
        .filter_map(|(k, v)| {
            let (key, key_node) = match k {
                Node::Scalar(s) => (s.decoded().ok()?, s.syntax().clone()),
                _ => return None,
            };
            let v = v?;
            Some((key, key_node, node_syntax(&v)))
        })
        .collect()
}

impl IntoIterator for StreamCursor {
    type Item = Dynamic;
    type IntoIter = CursorIter;
    fn into_iter(self) -> Self::IntoIter {
        let stream = crate::ast::Stream::cast(self.node.clone());
        let items: Vec<Dynamic> = match stream {
            Some(st) => st
                .documents()
                .filter_map(|d| d.root_node())
                .map(|n| cursor_for(Rc::clone(&self.tree), node_syntax(&n)))
                .collect(),
            None => Vec::new(),
        };
        CursorIter {
            items: items.into_iter(),
        }
    }
}

/// Extract the byte span of any cursor stored in a `Dynamic`. Returns
/// `None` for non-cursor values; returns `Some(0..0)` for the null cursor
/// (empty document) so callers can still record a finding.
pub fn span_of_dynamic(d: &Dynamic) -> Option<Range<usize>> {
    let span_of = |n: &SyntaxNode| {
        let r = n.text_range();
        usize::from(r.start())..usize::from(r.end())
    };
    if let Some(c) = d.read_lock::<MappingCursor>() {
        return Some(span_of(&c.node));
    }
    if let Some(c) = d.read_lock::<SequenceCursor>() {
        return Some(span_of(&c.node));
    }
    if let Some(c) = d.read_lock::<ScalarCursor>() {
        return Some(span_of(&c.node));
    }
    if let Some(c) = d.read_lock::<StreamCursor>() {
        return Some(span_of(&c.node));
    }
    if d.read_lock::<NullCursor>().is_some() {
        return Some(0..0);
    }
    None
}

fn value_to_dynamic(v: &Value) -> Dynamic {
    match v {
        Value::Null => Dynamic::UNIT,
        Value::Bool(b) => Dynamic::from(*b),
        Value::Int(i) => Dynamic::from(*i),
        Value::Float(f) => Dynamic::from(*f),
        Value::String(s) => Dynamic::from(s.clone()),
    }
}

fn dynamic_to_yaml_scalar_text(d: &Dynamic) -> Option<String> {
    if d.is_unit() {
        return Some("null".to_string());
    }
    if let Some(b) = d.clone().try_cast::<bool>() {
        return Some(b.to_string());
    }
    if let Some(i) = d.clone().try_cast::<i64>() {
        return Some(i.to_string());
    }
    if let Some(f) = d.clone().try_cast::<f64>() {
        return Some(f.to_string());
    }
    if let Some(s) = d.clone().try_cast::<String>() {
        return Some(yaml_quote_if_needed(&s));
    }
    if let Some(s) = d.clone().try_cast::<ImmutableString>() {
        return Some(yaml_quote_if_needed(&s));
    }
    None
}

fn yaml_quote_if_needed(s: &str) -> String {
    const SPECIAL_CHARS: &[char] = &['\n', '\r', '\t', ':', '#', '\'', '"'];
    const LEADING_INDICATORS: &[char] = &['-', '?', '*', '&', '!', '|', '>', '@', '%', '`'];
    let needs_quotes = s.is_empty()
        || s.contains(SPECIAL_CHARS)
        || s.starts_with(LEADING_INDICATORS)
        || matches!(
            s,
            "true" | "false" | "null" | "~" | "True" | "False" | "Null" | "TRUE" | "FALSE" | "NULL"
        )
        || is_ambiguous_number(s);
    if needs_quotes {
        let escaped = s.replace('\'', "''");
        format!("'{escaped}'")
    } else {
        s.to_string()
    }
}

fn is_ambiguous_number(s: &str) -> bool {
    s.parse::<i64>().is_ok() || s.parse::<f64>().is_ok()
}

#[export_module]
#[allow(non_snake_case)]
pub mod mapping_api {
    use super::*;

    /// Number of entries in the mapping.
    #[rhai_fn(pure)]
    pub fn len(m: &mut MappingCursor) -> i64 {
        Node::cast(m.node.clone())
            .as_ref()
            .and_then(Node::as_mapping)
            .map_or(0, |m| m.len() as i64)
    }

    /// Return the raw source text spanning this mapping.
    #[rhai_fn(name = "to_string", pure)]
    pub fn mapping_to_string(m: &mut MappingCursor) -> String {
        m.node.text().to_string()
    }

    /// Decoded keys of the mapping as an array of strings.
    #[rhai_fn(pure)]
    pub fn keys(m: &mut MappingCursor) -> rhai::Array {
        let keys: Vec<String> = Node::cast(m.node.clone())
            .as_ref()
            .and_then(Node::as_mapping)
            .map(|m| m.keys().collect())
            .unwrap_or_default();
        keys.into_iter().map(Dynamic::from).collect()
    }

    /// Value cursors of the mapping, in source order. Key-only entries
    /// are skipped.
    #[rhai_fn(pure)]
    pub fn values(m: &mut MappingCursor) -> rhai::Array {
        let nodes: Vec<SyntaxNode> = Node::cast(m.node.clone())
            .as_ref()
            .and_then(Node::as_mapping)
            .map(|m| m.values().map(|n| node_syntax(&n)).collect())
            .unwrap_or_default();
        nodes
            .into_iter()
            .map(|n| cursor_for(Rc::clone(&m.tree), n))
            .collect()
    }

    /// Indexer: `m[key]`.
    #[rhai_fn(index_get, return_raw, pure)]
    pub fn get_by_key(
        m: &mut MappingCursor,
        key: ImmutableString,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        map_step_key(m, &key)
    }

    /// Leading comment block attached to this mapping's enclosing entry
    /// (or document, at root). Empty string when absent.
    #[rhai_fn(name = "comment", pure)]
    pub fn mapping_comment(m: &mut MappingCursor) -> String {
        comment::read_leading(&m.node)
    }

    /// Replace the leading comment block with `text`. Multiline strings
    /// emit one `#`-prefixed line per `\n`-separated segment. Errors
    /// when the cursor is inside a flow-style construct.
    #[rhai_fn(name = "set_comment", return_raw)]
    pub fn mapping_set_comment(
        m: &mut MappingCursor,
        text: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        let mut tree = m.tree.borrow_mut();
        comment::write_leading(&mut tree, &m.node, &text).map_err(|e| e.into())
    }

    /// Remove the leading comment block if present.
    #[rhai_fn(name = "clear_comment")]
    pub fn mapping_clear_comment(m: &mut MappingCursor) {
        let mut tree = m.tree.borrow_mut();
        comment::clear_leading(&mut tree, &m.node);
    }

    /// Same-line trailing comment on this mapping's enclosing entry.
    #[rhai_fn(name = "trailing_comment", pure)]
    pub fn mapping_trailing_comment(m: &mut MappingCursor) -> String {
        comment::read_trailing(&m.node)
    }

    /// Set the same-line trailing comment. Errors if `text` has `\n`.
    #[rhai_fn(name = "set_trailing_comment", return_raw)]
    pub fn mapping_set_trailing_comment(
        m: &mut MappingCursor,
        text: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        let mut tree = m.tree.borrow_mut();
        comment::write_trailing(&mut tree, &m.node, &text).map_err(|e| e.into())
    }

    /// Remove the same-line trailing comment if present.
    #[rhai_fn(name = "clear_trailing_comment")]
    pub fn mapping_clear_trailing_comment(m: &mut MappingCursor) {
        let mut tree = m.tree.borrow_mut();
        comment::clear_trailing(&mut tree, &m.node);
    }

    /// Whether the mapping contains `key`.
    #[rhai_fn(name = "contains", pure)]
    pub fn mapping_contains(m: &mut MappingCursor, key: ImmutableString) -> bool {
        Node::cast(m.node.clone())
            .as_ref()
            .and_then(Node::as_mapping)
            .is_some_and(|m| m.get(&key).is_some())
    }

    /// Rename the entry whose key matches `old_key` to `new_key`. Returns
    /// `true` if the key was found and renamed, `false` if not found.
    /// Errors when the target key is not a plain scalar (e.g. quoted or
    /// complex key).
    #[rhai_fn(name = "rename_key", return_raw)]
    pub fn mapping_rename_key(
        m: &mut MappingCursor,
        old_key: ImmutableString,
        new_key: ImmutableString,
    ) -> Result<bool, Box<EvalAltResult>> {
        let Some(key_syntax) = mapping_key_node(&m.node, &old_key) else {
            return Ok(false);
        };

        let scalar = crate::ast::Scalar::cast(key_syntax.clone())
            .ok_or_else(|| "key is not a scalar node".to_string())?;
        let tok = scalar
            .value_token()
            .ok_or_else(|| "key scalar has no value token".to_string())?;

        let text = yaml_quote_if_needed(&new_key);
        let (token_kind, token_text) = if text.starts_with('\'') && text.ends_with('\'') {
            (SyntaxKind::SINGLE_QUOTED_SCALAR, text)
        } else {
            (SyntaxKind::PLAIN_SCALAR, text)
        };

        let mut tree = m.tree.borrow_mut();
        if tok.kind() == token_kind {
            apply_edit(
                &mut tree,
                TreeEdit::ReplaceToken {
                    target: tok,
                    new: mk_token(token_kind, &token_text),
                },
            );
        } else {
            let new_node = mk_node(
                SyntaxKind::SCALAR,
                [NodeOrToken::Token(mk_token(token_kind, &token_text))],
            );
            apply_edit(
                &mut tree,
                TreeEdit::ReplaceNode {
                    target: key_syntax,
                    new: new_node,
                },
            );
        }
        Ok(true)
    }
}

fn seq_elements_of_kind(s: &SequenceCursor, predicate: impl Fn(SyntaxKind) -> bool) -> rhai::Array {
    let children: Vec<SyntaxNode> = Node::cast(s.node.clone())
        .as_ref()
        .and_then(Node::as_sequence)
        .map(|s| s.iter().map(|n| node_syntax(&n)).collect())
        .unwrap_or_default();
    children
        .into_iter()
        .filter(|n| predicate(n.kind()))
        .map(|n| cursor_for(Rc::clone(&s.tree), n))
        .collect()
}

#[export_module]
#[allow(non_snake_case)]
pub mod sequence_api {
    use super::*;

    /// Number of elements in the sequence.
    #[rhai_fn(pure)]
    pub fn len(s: &mut SequenceCursor) -> i64 {
        Node::cast(s.node.clone())
            .as_ref()
            .and_then(Node::as_sequence)
            .map_or(0, |s| s.len() as i64)
    }

    /// Return the raw source text spanning this sequence.
    #[rhai_fn(name = "to_string", pure)]
    pub fn sequence_to_string(s: &mut SequenceCursor) -> String {
        s.node.text().to_string()
    }

    /// Indexer: `s[i]`.
    #[rhai_fn(index_get, return_raw, pure)]
    pub fn get_by_index(s: &mut SequenceCursor, i: i64) -> Result<Dynamic, Box<EvalAltResult>> {
        seq_step_index(s, i)
    }

    /// Leading comment block attached to this sequence's enclosing entry
    /// (or document, at root). Empty string when absent.
    #[rhai_fn(name = "comment", pure)]
    pub fn sequence_comment(s: &mut SequenceCursor) -> String {
        comment::read_leading(&s.node)
    }

    #[rhai_fn(name = "set_comment", return_raw)]
    pub fn sequence_set_comment(
        s: &mut SequenceCursor,
        text: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        let mut tree = s.tree.borrow_mut();
        comment::write_leading(&mut tree, &s.node, &text).map_err(|e| e.into())
    }

    #[rhai_fn(name = "clear_comment")]
    pub fn sequence_clear_comment(s: &mut SequenceCursor) {
        let mut tree = s.tree.borrow_mut();
        comment::clear_leading(&mut tree, &s.node);
    }

    #[rhai_fn(name = "trailing_comment", pure)]
    pub fn sequence_trailing_comment(s: &mut SequenceCursor) -> String {
        comment::read_trailing(&s.node)
    }

    #[rhai_fn(name = "set_trailing_comment", return_raw)]
    pub fn sequence_set_trailing_comment(
        s: &mut SequenceCursor,
        text: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        let mut tree = s.tree.borrow_mut();
        comment::write_trailing(&mut tree, &s.node, &text).map_err(|e| e.into())
    }

    #[rhai_fn(name = "clear_trailing_comment")]
    pub fn sequence_clear_trailing_comment(s: &mut SequenceCursor) {
        let mut tree = s.tree.borrow_mut();
        comment::clear_trailing(&mut tree, &s.node);
    }

    /// All elements that are mappings, in source order. Non-mapping
    /// elements are silently skipped.
    #[rhai_fn(pure)]
    pub fn mappings(s: &mut SequenceCursor) -> rhai::Array {
        seq_elements_of_kind(s, |k| {
            matches!(k, SyntaxKind::BLOCK_MAPPING | SyntaxKind::FLOW_MAPPING)
        })
    }

    /// All elements that are sequences, in source order. Non-sequence
    /// elements are silently skipped.
    #[rhai_fn(pure)]
    pub fn sequences(s: &mut SequenceCursor) -> rhai::Array {
        seq_elements_of_kind(s, |k| {
            matches!(k, SyntaxKind::BLOCK_SEQUENCE | SyntaxKind::FLOW_SEQUENCE)
        })
    }

    /// All elements that are scalars, in source order. Non-scalar
    /// elements are silently skipped.
    #[rhai_fn(pure)]
    pub fn scalars(s: &mut SequenceCursor) -> rhai::Array {
        seq_elements_of_kind(s, |k| matches!(k, SyntaxKind::SCALAR | SyntaxKind::ALIAS))
    }
}

#[export_module]
#[allow(non_snake_case)]
pub mod scalar_api {
    use super::*;

    /// Decode the scalar to its YAML Core schema value.
    #[rhai_fn(return_raw, pure)]
    pub fn read(s: &mut ScalarCursor) -> Result<Dynamic, Box<EvalAltResult>> {
        let scalar = Scalar::cast(s.node.clone()).ok_or_else(|| "not a scalar".to_string())?;
        let v = scalar
            .to_value()
            .map_err(|e| format!("decode error: {e:?}"))?;
        Ok(value_to_dynamic(&v))
    }

    /// Replace the scalar's value with `value`. Kind of the underlying
    /// token is preserved; text is encoded via yaml-safe quoting rules.
    #[rhai_fn(return_raw)]
    pub fn write(s: &mut ScalarCursor, value: Dynamic) -> Result<(), Box<EvalAltResult>> {
        let text = dynamic_to_yaml_scalar_text(&value)
            .ok_or_else(|| "cannot write this value as a YAML scalar".to_string())?;
        // Determine the right token kind. If the scalar is currently
        // PLAIN_SCALAR and the encoded text doesn't start with a quote,
        // keep PLAIN_SCALAR; if it starts with `'`, use SINGLE_QUOTED_SCALAR.
        let (token_kind, token_text) = if text.starts_with('\'') && text.ends_with('\'') {
            (SyntaxKind::SINGLE_QUOTED_SCALAR, text.clone())
        } else {
            (SyntaxKind::PLAIN_SCALAR, text.clone())
        };
        let scalar = Scalar::cast(s.node.clone()).ok_or_else(|| "not a scalar".to_string())?;
        let Some(current_tok) = scalar.value_token() else {
            return Err("scalar has no value token".into());
        };

        let mut tree = s.tree.borrow_mut();
        if current_tok.kind() == token_kind {
            // Same kind: token-level replace.
            let new = mk_token(token_kind, &token_text);
            apply_edit(
                &mut tree,
                TreeEdit::ReplaceToken {
                    target: current_tok,
                    new,
                },
            );
        } else {
            // Kind change: rebuild the enclosing SCALAR node.
            let new_node = mk_node(
                SyntaxKind::SCALAR,
                [NodeOrToken::Token(mk_token(token_kind, &token_text))],
            );
            apply_edit(
                &mut tree,
                TreeEdit::ReplaceNode {
                    target: s.node.clone(),
                    new: new_node,
                },
            );
        }
        Ok(())
    }

    /// Raw source text of the scalar.
    #[rhai_fn(name = "to_string", pure)]
    pub fn scalar_to_string(s: &mut ScalarCursor) -> String {
        s.node.text().to_string()
    }

    /// Leading comment block attached to this scalar's enclosing entry.
    #[rhai_fn(name = "comment", pure)]
    pub fn scalar_comment(s: &mut ScalarCursor) -> String {
        comment::read_leading(&s.node)
    }

    #[rhai_fn(name = "set_comment", return_raw)]
    pub fn scalar_set_comment(
        s: &mut ScalarCursor,
        text: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        let mut tree = s.tree.borrow_mut();
        comment::write_leading(&mut tree, &s.node, &text).map_err(|e| e.into())
    }

    #[rhai_fn(name = "clear_comment")]
    pub fn scalar_clear_comment(s: &mut ScalarCursor) {
        let mut tree = s.tree.borrow_mut();
        comment::clear_leading(&mut tree, &s.node);
    }

    #[rhai_fn(name = "trailing_comment", pure)]
    pub fn scalar_trailing_comment(s: &mut ScalarCursor) -> String {
        comment::read_trailing(&s.node)
    }

    #[rhai_fn(name = "set_trailing_comment", return_raw)]
    pub fn scalar_set_trailing_comment(
        s: &mut ScalarCursor,
        text: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        let mut tree = s.tree.borrow_mut();
        comment::write_trailing(&mut tree, &s.node, &text).map_err(|e| e.into())
    }

    #[rhai_fn(name = "clear_trailing_comment")]
    pub fn scalar_clear_trailing_comment(s: &mut ScalarCursor) {
        let mut tree = s.tree.borrow_mut();
        comment::clear_trailing(&mut tree, &s.node);
    }

    /// Try to rewrite this scalar as an unquoted (plain) scalar. Returns
    /// `true` if the rewrite was applied, `false` if the value cannot be
    /// represented without quotes.
    #[rhai_fn(name = "try_make_unquoted")]
    pub fn scalar_try_make_unquoted(s: &mut ScalarCursor) -> bool {
        let scalar = match Scalar::cast(s.node.clone()) {
            Some(sc) => sc,
            None => return false,
        };
        let tok = match scalar.value_token() {
            Some(t) => t,
            None => return false,
        };
        let inner = match tok.kind() {
            SyntaxKind::SINGLE_QUOTED_SCALAR => {
                let raw = tok.text().trim_matches('\'');
                if raw.contains("''") {
                    return false;
                }
                raw.to_string()
            }
            SyntaxKind::DOUBLE_QUOTED_SCALAR => {
                let raw = tok.text().trim_matches('"');
                if raw.contains('\\') {
                    return false;
                }
                raw.to_string()
            }
            _ => return false,
        };
        let plain = match as_plain_scalar(&inner) {
            Some(p) => p.to_string(),
            None => return false,
        };
        let new = mk_node(
            SyntaxKind::SCALAR,
            [NodeOrToken::Token(mk_token(
                SyntaxKind::PLAIN_SCALAR,
                &plain,
            ))],
        );
        let mut tree = s.tree.borrow_mut();
        apply_edit(
            &mut tree,
            TreeEdit::ReplaceNode {
                target: s.node.clone(),
                new,
            },
        );
        true
    }

    /// Rewrite this scalar as an unquoted (plain) scalar. Errors if the
    /// value cannot be represented without quotes.
    #[rhai_fn(name = "make_unquoted", return_raw)]
    pub fn scalar_make_unquoted(s: &mut ScalarCursor) -> Result<(), Box<EvalAltResult>> {
        if scalar_try_make_unquoted(s) {
            Ok(())
        } else {
            Err("scalar cannot be represented as an unquoted value".into())
        }
    }

    /// Try to rewrite this scalar as a single-quoted scalar. Returns
    /// `true` if the rewrite was applied, `false` if the value contains
    /// characters that cannot appear in a single-quoted scalar (control
    /// characters) or is already single-quoted.
    #[rhai_fn(name = "try_make_single_quoted")]
    pub fn scalar_try_make_single_quoted(s: &mut ScalarCursor) -> bool {
        let scalar = match Scalar::cast(s.node.clone()) {
            Some(sc) => sc,
            None => return false,
        };
        let tok = match scalar.value_token() {
            Some(t) => t,
            None => return false,
        };
        if tok.kind() == SyntaxKind::SINGLE_QUOTED_SCALAR {
            return false;
        }
        let inner = match tok.kind() {
            SyntaxKind::DOUBLE_QUOTED_SCALAR => {
                let raw = tok.text().trim_matches('"');
                if raw.contains('\\') {
                    return false;
                }
                raw.to_string()
            }
            SyntaxKind::PLAIN_SCALAR => tok.text().to_string(),
            _ => return false,
        };
        let quoted = match as_single_quoted(&inner) {
            Some(q) => q,
            None => return false,
        };
        let new = mk_node(
            SyntaxKind::SCALAR,
            [NodeOrToken::Token(mk_token(
                SyntaxKind::SINGLE_QUOTED_SCALAR,
                &quoted,
            ))],
        );
        let mut tree = s.tree.borrow_mut();
        apply_edit(
            &mut tree,
            TreeEdit::ReplaceNode {
                target: s.node.clone(),
                new,
            },
        );
        true
    }

    /// Rewrite this scalar as a single-quoted scalar. Errors if the value
    /// contains characters that cannot appear in a single-quoted scalar.
    #[rhai_fn(name = "make_single_quoted", return_raw)]
    pub fn scalar_make_single_quoted(s: &mut ScalarCursor) -> Result<(), Box<EvalAltResult>> {
        if scalar_try_make_single_quoted(s) {
            Ok(())
        } else {
            Err("scalar cannot be represented as a single-quoted value".into())
        }
    }
}

#[export_module]
#[allow(non_snake_case)]
pub mod stream_api {
    use super::*;
    use crate::ast::Stream;

    /// Number of documents in the stream.
    #[rhai_fn(pure)]
    pub fn len(s: &mut StreamCursor) -> i64 {
        Stream::cast(s.node.clone())
            .map(|st| st.len() as i64)
            .unwrap_or(0)
    }

    /// `stream[i]` — cursor at the i-th document's root node.
    #[rhai_fn(index_get, return_raw, pure)]
    pub fn get_by_index(s: &mut StreamCursor, i: i64) -> Result<Dynamic, Box<EvalAltResult>> {
        if i < 0 {
            return Err(format!("negative index: {i}").into());
        }
        let stream = Stream::cast(s.node.clone()).ok_or_else(|| "not a stream".to_string())?;
        let doc = stream
            .documents()
            .nth(i as usize)
            .ok_or_else(|| format!("document index out of range: {i}"))?;
        let root = doc
            .root_node()
            .ok_or_else(|| format!("document {i} is empty"))?;
        Ok(cursor_for(Rc::clone(&s.tree), node_syntax(&root)))
    }

    /// Stream-level leading comment (before the first `---` marker).
    /// Empty when the stream starts with an implicit document.
    #[rhai_fn(name = "comment", pure)]
    pub fn stream_comment(s: &mut StreamCursor) -> String {
        Stream::cast(s.node.clone())
            .map(|st| st.leading_comment())
            .unwrap_or_default()
    }

    /// Replace the stream-level leading comment with `text`. Errors
    /// when the stream has no explicit `---`.
    #[rhai_fn(name = "set_comment", return_raw)]
    pub fn stream_set_comment(
        s: &mut StreamCursor,
        text: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        let stream = Stream::cast(s.node.clone()).ok_or_else(|| "not a stream".to_string())?;
        let mut tree = s.tree.borrow_mut();
        stream
            .set_leading_comment(&mut tree, Some(&text))
            .map_err(|e| e.into())
    }

    #[rhai_fn(name = "clear_comment", return_raw)]
    pub fn stream_clear_comment(s: &mut StreamCursor) -> Result<(), Box<EvalAltResult>> {
        let stream = Stream::cast(s.node.clone()).ok_or_else(|| "not a stream".to_string())?;
        let mut tree = s.tree.borrow_mut();
        stream
            .set_leading_comment(&mut tree, None)
            .map_err(|e| e.into())
    }

    /// Raw source of the stream (entire document text).
    #[rhai_fn(name = "to_string", pure)]
    pub fn stream_to_string(s: &mut StreamCursor) -> String {
        s.node.text().to_string()
    }

    /// Run the built-in formatter over the whole stream in place.
    /// Normalizes indentation, trims trailing whitespace, and
    /// single-quotes plain scalars that collide with YAML 1.1 boolean
    /// words. No-op when the document is already clean.
    #[rhai_fn(name = "fmt")]
    pub fn stream_fmt(s: &mut StreamCursor) {
        let mut tree = s.tree.borrow_mut();
        use crate::fixers;
        fixers::reindent(&mut tree);
        fixers::trim_trailing_whitespace(&mut tree);
    }

    /// Normalize indentation throughout the stream. Idempotent.
    #[rhai_fn(name = "reindent")]
    pub fn stream_reindent(s: &mut StreamCursor) {
        crate::fixers::reindent(&mut s.tree.borrow_mut());
    }

    /// Remove trailing whitespace from all lines in the stream. Idempotent.
    #[rhai_fn(name = "trim_trailing")]
    pub fn stream_trim_trailing(s: &mut StreamCursor) {
        crate::fixers::trim_trailing_whitespace(&mut s.tree.borrow_mut());
    }

    /// Remove unnecessary quotes from all quoted scalars in the stream
    /// whose content is safe as a plain scalar. Idempotent.
    #[rhai_fn(name = "make_unquoted")]
    pub fn stream_make_unquoted(s: &mut StreamCursor) {
        let mut tree = s.tree.borrow_mut();
        crate::fixers::unquote_scalars(&mut tree);
    }

    /// Convert double-quoted scalars to single-quoted throughout the
    /// stream where possible. Idempotent.
    #[rhai_fn(name = "make_single_quoted")]
    pub fn stream_make_single_quoted(s: &mut StreamCursor) {
        let mut tree = s.tree.borrow_mut();
        crate::fixers::prefer_single_quotes(&mut tree);
    }

    /// Apply YAML 1.1 → 1.2 safety fixes over the whole stream in
    /// place: rewrites plain-scalar booleans (`yes/no/on/off`), octals
    /// (`0777`), and sexagesimals (`1:30:00`) to keep their meaning
    /// under YAML 1.2 semantics. Idempotent.
    #[rhai_fn(name = "fix")]
    pub fn stream_fix(s: &mut StreamCursor) {
        let mut tree = s.tree.borrow_mut();
        use crate::fixers;
        fixers::yaml11_bool_to_yaml12_bool(&mut tree);
        fixers::yaml11_octal_to_quoted(&mut tree);
        fixers::yaml11_sexagesimal_to_quoted(&mut tree);
    }
}

#[export_module]
#[allow(non_snake_case)]
pub mod map_entry_api {
    use super::*;

    /// Decoded key string.
    #[rhai_fn(pure)]
    pub fn key(e: &mut MapEntry) -> String {
        e.key.clone()
    }

    /// Value cursor.
    #[rhai_fn(pure)]
    pub fn value(e: &mut MapEntry) -> Dynamic {
        e.value.clone()
    }

    /// Scalar cursor pointing at the entry's key, suitable for
    /// passing to `file.lint(...)`.
    #[rhai_fn(name = "key_node", pure)]
    pub fn key_node(e: &mut MapEntry) -> Dynamic {
        e.key_cursor()
    }
}

#[export_module]
pub mod null_api {
    use super::*;

    #[rhai_fn(pure)]
    pub fn len(_: &mut NullCursor) -> i64 {
        0
    }
    #[rhai_fn(pure)]
    pub fn keys(_: &mut NullCursor) -> rhai::Array {
        vec![]
    }
    #[rhai_fn(pure)]
    pub fn values(_: &mut NullCursor) -> rhai::Array {
        vec![]
    }
    #[rhai_fn(pure)]
    pub fn mappings(_: &mut NullCursor) -> rhai::Array {
        vec![]
    }
    #[rhai_fn(pure)]
    pub fn sequences(_: &mut NullCursor) -> rhai::Array {
        vec![]
    }
    #[rhai_fn(pure)]
    pub fn scalars(_: &mut NullCursor) -> rhai::Array {
        vec![]
    }
    #[rhai_fn(name = "contains", pure)]
    pub fn null_contains(_: &mut NullCursor, _: ImmutableString) -> bool {
        false
    }
    #[rhai_fn(name = "to_string", pure)]
    pub fn null_to_string(_: &mut NullCursor) -> String {
        String::new()
    }
    #[rhai_fn(name = "comment", pure)]
    pub fn null_comment(_: &mut NullCursor) -> String {
        String::new()
    }
    #[rhai_fn(name = "trailing_comment", pure)]
    pub fn null_trailing_comment(_: &mut NullCursor) -> String {
        String::new()
    }
}

// ─── YamlFile ────────────────────────────────────────────────────────────────

struct ParsedFile {
    tree: TreeHandle,
}

/// Rhai-exposed wrapper for a single YAML file. Both reading the
/// source and parsing it are deferred until the script first touches
/// `.stream` / `.document` / `.documents` (or one of the in-place
/// editing methods). Path properties (`path`, `basename`, `dirname`,
/// `stem`, `extension`) are always available immediately.
///
/// `real_path` is `Some(...)` when the file was loaded from disk
/// (e.g. via `input.glob(...)` under `ito run`). It is a
/// canonical absolute path that has been verified to live inside the
/// runner's input root at construction time. It is absent for
/// in-memory files (e.g. those built by `make_yaml_file` in the
/// registered-lint runner) — those carry their `source` inline.
///
/// Parse failures propagate to the script as errors. Skipping a
/// problematic file is the script's job: filter on the path
/// properties before touching the tree.
#[derive(Clone)]
pub struct YamlFile {
    path: String,
    /// Inline source for in-memory files. `None` means "read from
    /// `real_path` on first access". Materialized exactly once so
    /// repeated `take_new_source` calls don't re-read disk.
    source: Rc<RefCell<Option<String>>>,
    real_path: Option<std::path::PathBuf>,
    inner: Rc<RefCell<Option<ParsedFile>>>,
}

impl YamlFile {
    /// Read the file's source, lazy on first call. Returns the cached
    /// string thereafter. Disk read failures bubble up as Rhai errors.
    fn ensure_source(&self) -> Result<(), Box<EvalAltResult>> {
        if self.source.borrow().is_some() {
            return Ok(());
        }
        let Some(real) = self.real_path.as_ref() else {
            // No inline source and no on-disk path is a programmer
            // error in the runner — every YamlFile must have one.
            return Err(format!("file {} has no source available", self.path).into());
        };
        let text =
            std::fs::read_to_string(real).map_err(|e| format!("read error {}: {e}", self.path))?;
        *self.source.borrow_mut() = Some(text);
        Ok(())
    }

    fn ensure_parsed(&self) -> Result<(), Box<EvalAltResult>> {
        if self.inner.borrow().is_some() {
            return Ok(());
        }
        self.ensure_source()?;
        let guard = self.source.borrow();
        let src = guard.as_ref().unwrap();
        let tree = crate::parse(src).map_err(|e| format!("parse error in {}: {e}", self.path))?;
        let tree_cell: TreeHandle = Rc::new(RefCell::new(tree));
        drop(guard);
        *self.inner.borrow_mut() = Some(ParsedFile { tree: tree_cell });
        Ok(())
    }

    /// Public wrapper around `ensure_parsed` so callers outside this
    /// module (e.g. the `file.lint()` closure in `cmd/run.rs`) can
    /// trigger parsing before walking allow-directive metadata.
    pub fn ensure_parsed_pub(&self) -> Result<(), Box<EvalAltResult>> {
        self.ensure_parsed()
    }

    /// Display path of this file (absolute disk path when loaded via
    /// `glob()` / `list_files()` under `ito run`).
    pub fn path_str(&self) -> &str {
        &self.path
    }

    /// On-disk path the file was loaded from, if any. `None` for
    /// in-memory files (e.g. those created by `make_yaml_file`).
    pub fn real_path(&self) -> Option<&std::path::Path> {
        self.real_path.as_deref()
    }

    /// Originally loaded source text. `None` when the file hasn't
    /// been read yet (i.e. the script never touched the document).
    /// Used for rendering lint findings against the snapshot the
    /// script saw, even after mutations.
    pub fn source_snapshot(&self) -> Option<String> {
        self.source.borrow().clone()
    }

    /// Emit the (possibly mutated) source text. `None` when the file
    /// was never parsed — without a tree there's nothing to emit and
    /// nothing to write back.
    pub fn take_new_source(&self) -> Option<String> {
        self.inner.borrow().as_ref().map(|p| p.tree.borrow().emit())
    }

    /// Build the `(document, stream)` cursor pair for this file, used
    /// by the runner to bind `d`/`doc`/`document`/`s`/`stream` in file
    /// invocations. Both values are typed cursors (or `()` if the
    /// document is empty / the parsed tree has no root). Caller is
    /// expected to have called `ensure_parsed_pub()` already.
    pub fn root_cursors(&self) -> (Dynamic, Dynamic) {
        let guard = self.inner.borrow();
        let Some(p) = guard.as_ref() else {
            return (Dynamic::UNIT, Dynamic::UNIT);
        };
        let doc = root_cursor(Rc::clone(&p.tree)).unwrap_or(Dynamic::UNIT);
        let stream_node = p.tree.borrow().root().clone();
        let stream = Dynamic::from(StreamCursor {
            tree: Rc::clone(&p.tree),
            node: stream_node,
        });
        (doc, stream)
    }
}

/// Build an in-memory `YamlFile` whose source is supplied inline.
/// Used for stdin where the source has already been read.
pub(crate) fn make_yaml_file(path: String, source: String) -> YamlFile {
    YamlFile {
        path,
        source: Rc::new(RefCell::new(Some(source))),
        real_path: None,
        inner: Rc::new(RefCell::new(None)),
    }
}

/// Build a disk-backed `YamlFile` whose source is read on first
/// access. Returned by `input.glob(...)`; lazy so a script can
/// filter on path properties (e.g. `parsed.stem`) without paying for
/// reading or parsing files it ends up skipping.
pub(crate) fn make_yaml_file_lazy(path: String, real_path: std::path::PathBuf) -> YamlFile {
    YamlFile {
        path,
        source: Rc::new(RefCell::new(None)),
        real_path: Some(real_path),
        inner: Rc::new(RefCell::new(None)),
    }
}

/// Build a disk-backed `YamlFile` with the source preloaded.
pub(crate) fn make_yaml_file_with_real_path(
    path: String,
    source: String,
    real_path: std::path::PathBuf,
) -> YamlFile {
    YamlFile {
        path,
        source: Rc::new(RefCell::new(Some(source))),
        real_path: Some(real_path),
        inner: Rc::new(RefCell::new(None)),
    }
}

#[export_module]
#[allow(non_snake_case)]
pub mod yaml_file_api {
    use super::*;
    use crate::ast::Stream;

    /// Display path of the file (`/dev/stdin` for stdin input, otherwise
    /// the absolute disk path). Also callable as `file.path()` for
    /// backward compatibility with shipped lint scripts.
    #[rhai_fn(get = "path", name = "path", pure)]
    pub fn path(f: &mut YamlFile) -> String {
        f.path.clone()
    }

    /// Last path component (`config.yml` for `/etc/foo/config.yml`).
    #[rhai_fn(get = "basename", name = "basename", pure)]
    pub fn basename(f: &mut YamlFile) -> String {
        std::path::Path::new(&f.path)
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_default()
    }

    /// Parent directory (`/etc/foo` for `/etc/foo/config.yml`).
    #[rhai_fn(get = "dirname", name = "dirname", pure)]
    pub fn dirname(f: &mut YamlFile) -> String {
        std::path::Path::new(&f.path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    /// Filename without extension (`config` for `config.yml`).
    #[rhai_fn(get = "stem", name = "stem", pure)]
    pub fn stem(f: &mut YamlFile) -> String {
        std::path::Path::new(&f.path)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_default()
    }

    /// Filename extension without the dot (`yml` for `config.yml`).
    #[rhai_fn(get = "extension", name = "extension", pure)]
    pub fn extension(f: &mut YamlFile) -> String {
        std::path::Path::new(&f.path)
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_default()
    }

    /// Full stream cursor (every document). Triggers a lazy
    /// read+parse on first call; parse failures abort the script.
    /// Also callable as `file.stream()`.
    #[rhai_fn(get = "stream", name = "stream", return_raw, pure)]
    pub fn stream(f: &mut YamlFile) -> Result<Dynamic, Box<EvalAltResult>> {
        f.ensure_parsed()?;
        let guard = f.inner.borrow();
        let p = guard.as_ref().unwrap();
        Ok(Dynamic::from(StreamCursor {
            tree: Rc::clone(&p.tree),
            node: p.tree.borrow().root().clone(),
        }))
    }

    /// First document's root cursor. Triggers a lazy read+parse on
    /// first call; parse failures abort the script. Returns `()` for
    /// an empty file. Also callable as `file.document()`.
    #[rhai_fn(get = "document", name = "document", return_raw, pure)]
    pub fn document(f: &mut YamlFile) -> Result<Dynamic, Box<EvalAltResult>> {
        f.ensure_parsed()?;
        let guard = f.inner.borrow();
        let p = guard.as_ref().unwrap();
        Ok(root_cursor(Rc::clone(&p.tree)).unwrap_or(Dynamic::UNIT))
    }

    /// `true` if the file was parsed and any mutation changed its content.
    /// `false` if the file was never touched or the script wrote back
    /// the same bytes it read.
    #[rhai_fn(get = "changed", name = "changed", pure)]
    pub fn changed(f: &mut YamlFile) -> bool {
        match (f.take_new_source(), f.source_snapshot()) {
            (Some(new), Some(original)) => new != original,
            _ => false,
        }
    }

    /// Every document's root cursor as an indexable + iterable array.
    /// Triggers a lazy read+parse on first call; parse failures abort
    /// the script. Also callable as `file.documents()`.
    #[rhai_fn(get = "documents", name = "documents", return_raw, pure)]
    pub fn documents(f: &mut YamlFile) -> Result<rhai::Array, Box<EvalAltResult>> {
        f.ensure_parsed()?;
        let guard = f.inner.borrow();
        let p = guard.as_ref().unwrap();
        let Some(stream) = Stream::cast(p.tree.borrow().root().clone()) else {
            return Ok(rhai::Array::new());
        };
        Ok(stream
            .documents()
            .filter_map(|d| d.root_node())
            .map(|n| cursor_for(Rc::clone(&p.tree), node_syntax(&n)))
            .collect())
    }
}

pub fn register(engine: &mut rhai::Engine) {
    engine.register_type_with_name::<NullCursor>("YamlNull");
    engine.register_type_with_name::<MappingCursor>("YamlMapping");
    engine.register_type_with_name::<SequenceCursor>("YamlSequence");
    engine.register_type_with_name::<ScalarCursor>("YamlScalar");
    engine.register_type_with_name::<StreamCursor>("YamlStream");
    engine.register_type_with_name::<MapEntry>("YamlEntry");
    engine.register_type_with_name::<YamlFile>("YamlFile");

    engine.register_global_module(exported_module!(null_api).into());
    engine.register_global_module(exported_module!(mapping_api).into());
    engine.register_global_module(exported_module!(sequence_api).into());
    engine.register_global_module(exported_module!(scalar_api).into());
    engine.register_global_module(exported_module!(stream_api).into());
    engine.register_global_module(exported_module!(map_entry_api).into());
    engine.register_global_module(exported_module!(yaml_file_api).into());

    engine.register_iterator::<SequenceCursor>();
    engine.register_iterator::<MappingCursor>();
    engine.register_iterator::<StreamCursor>();

    // Property-style `.key` navigation on mappings — wired generically
    // so `d.foo` resolves via indexer_get. Rhai's property access falls
    // back to indexer_get when `get$foo` isn't registered; we register
    // indexer_get with String keys above.
}
