//! Format-preserving YAML editing for the Rhai surface.
//!
//! `yaml::edit(text)` parses the source into a shared, mutable concrete
//! syntax tree (`yamlito::SyntaxTree`) and hands back a [`StreamCursor`]
//! over its root. The `.doc` property steps to the first document's root
//! cursor (erroring on an empty stream); further navigation calls
//! (`stream[0]`, `m[key]`, `s[i]`) resolve a step against the live tree
//! and fail immediately with a clear error if the path doesn't exist.
//! Edits (`write`, `set_comment`, `rename_key`, `make_single_quoted`, …)
//! mutate the tree in place. `.to_string()` renders the (possibly
//! mutated) source, layout and comments intact — unlike the lossy
//! `yaml::parse`/`yaml::to_string` round-trip.
//!
//! Each cursor is a separate Rust type registered with Rhai. That gives
//! shift-left error reporting: calling `.read()` on a mapping surfaces as
//! a Rhai "method not found for YamlMapping" at dispatch, before any
//! handler code runs. There is no `Missing` cursor or lazy-chain.
//!
//! All cursors share one `Rc<RefCell<SyntaxTree>>`, so mutations made
//! through one are visible through the others (and through the original
//! stream handle the script holds).
//!
//! Ported from the `yamlito` crate's `scripting` glue, dropping the
//! file/input/lint scaffolding that belonged to a previous runner.

use std::cell::RefCell;
use std::rc::Rc;

use rhai::plugin::*;
use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Module};

use yamlito::SyntaxTree;
use yamlito::ast::{AstNode, Mapping, Node, Scalar, Value};
use yamlito::edit::{GreenElement, TreeEdit, apply_edit, mk_node, mk_token};
use yamlito::lexer::{as_plain_scalar, as_single_quoted};
use yamlito::syntax::{SyntaxKind, SyntaxNode};

use super::comment;

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
    /// Cursor (scalar) at the entry's key node.
    fn key_cursor(&self) -> Dynamic {
        cursor_for(Rc::clone(&self.tree), self.key_node.clone())
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
        let stream = yamlito::ast::Stream::cast(self.node.clone());
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

        let scalar = yamlito::ast::Scalar::cast(key_syntax.clone())
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
                [GreenElement::Token(mk_token(token_kind, &token_text))],
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
                [GreenElement::Token(mk_token(token_kind, &token_text))],
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
            [GreenElement::Token(mk_token(
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
            [GreenElement::Token(mk_token(
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
    use yamlito::ast::Stream;

    /// Number of documents in the stream.
    #[rhai_fn(pure)]
    pub fn len(s: &mut StreamCursor) -> i64 {
        Stream::cast(s.node.clone())
            .map(|st| st.len() as i64)
            .unwrap_or(0)
    }

    /// First document's root cursor, as the `doc` property. Errors for an
    /// empty stream (no documents, or a single empty document).
    #[rhai_fn(get = "doc", return_raw, pure)]
    pub fn doc(s: &mut StreamCursor) -> Result<Dynamic, Box<EvalAltResult>> {
        let root = Stream::cast(s.node.clone())
            .and_then(|st| st.documents().next())
            .and_then(|d| d.root_node())
            .ok_or_else(|| "stream has no document".to_string())?;
        Ok(cursor_for(Rc::clone(&s.tree), node_syntax(&root)))
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
        s.tree.borrow().emit()
    }

    // Whole-stream fixers are temporarily unregistered. The per-scalar
    // style helpers on `ScalarCursor` (make_unquoted / make_single_quoted
    // and their `try_` variants) remain available.
    //
    // /// Run the built-in formatter over the whole stream in place.
    // /// Normalizes indentation, trims trailing whitespace, and
    // /// single-quotes plain scalars that collide with YAML 1.1 boolean
    // /// words. No-op when the document is already clean.
    // #[rhai_fn(name = "fmt")]
    // pub fn stream_fmt(s: &mut StreamCursor) {
    //     let mut tree = s.tree.borrow_mut();
    //     use yamlito::fixers;
    //     fixers::reindent(&mut tree);
    //     fixers::trim_trailing_whitespace(&mut tree);
    // }
    //
    // /// Normalize indentation throughout the stream. Idempotent.
    // #[rhai_fn(name = "reindent")]
    // pub fn stream_reindent(s: &mut StreamCursor) {
    //     yamlito::fixers::reindent(&mut s.tree.borrow_mut());
    // }
    //
    // /// Remove trailing whitespace from all lines in the stream. Idempotent.
    // #[rhai_fn(name = "trim_trailing")]
    // pub fn stream_trim_trailing(s: &mut StreamCursor) {
    //     yamlito::fixers::trim_trailing_whitespace(&mut s.tree.borrow_mut());
    // }
    //
    // /// Remove unnecessary quotes from all quoted scalars in the stream
    // /// whose content is safe as a plain scalar. Idempotent.
    // #[rhai_fn(name = "make_unquoted")]
    // pub fn stream_make_unquoted(s: &mut StreamCursor) {
    //     let mut tree = s.tree.borrow_mut();
    //     yamlito::fixers::unquote_scalars(&mut tree);
    // }
    //
    // /// Convert double-quoted scalars to single-quoted throughout the
    // /// stream where possible. Idempotent.
    // #[rhai_fn(name = "make_single_quoted")]
    // pub fn stream_make_single_quoted(s: &mut StreamCursor) {
    //     let mut tree = s.tree.borrow_mut();
    //     yamlito::fixers::prefer_single_quotes(&mut tree);
    // }
    //
    // /// Apply YAML 1.1 → 1.2 safety fixes over the whole stream in
    // /// place: rewrites plain-scalar booleans (`yes/no/on/off`), octals
    // /// (`0777`), and sexagesimals (`1:30:00`) to keep their meaning
    // /// under YAML 1.2 semantics. Idempotent.
    // #[rhai_fn(name = "fix")]
    // pub fn stream_fix(s: &mut StreamCursor) {
    //     let mut tree = s.tree.borrow_mut();
    //     use yamlito::fixers;
    //     fixers::yaml11_bool_to_yaml12_bool(&mut tree);
    //     fixers::yaml11_octal_to_quoted(&mut tree);
    //     fixers::yaml11_sexagesimal_to_quoted(&mut tree);
    // }
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

    /// Scalar cursor pointing at the entry's key.
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

/// Parse `text` into a shared mutable tree and return a `StreamCursor`
/// over its root. Parse failures surface as Rhai runtime errors.
pub fn edit(text: &str) -> Result<StreamCursor, Box<EvalAltResult>> {
    let tree = yamlito::parse(text).map_err(|e| format!("yaml parse error: {e}"))?;
    let tree: TreeHandle = Rc::new(RefCell::new(tree));
    let node = tree.borrow().root().clone();
    Ok(StreamCursor { tree, node })
}

/// Register the editable YAML cursor types and their methods, plus the
/// `yaml::edit` entry point, on `engine`.
pub fn register(engine: &mut Engine, module: &mut Module) {
    engine.register_type_with_name::<NullCursor>("YamlNull");
    engine.register_type_with_name::<MappingCursor>("YamlMapping");
    engine.register_type_with_name::<SequenceCursor>("YamlSequence");
    engine.register_type_with_name::<ScalarCursor>("YamlScalar");
    engine.register_type_with_name::<StreamCursor>("YamlStream");
    engine.register_type_with_name::<MapEntry>("YamlEntry");

    engine.register_global_module(exported_module!(null_api).into());
    engine.register_global_module(exported_module!(mapping_api).into());
    engine.register_global_module(exported_module!(sequence_api).into());
    engine.register_global_module(exported_module!(scalar_api).into());
    engine.register_global_module(exported_module!(stream_api).into());
    engine.register_global_module(exported_module!(map_entry_api).into());

    engine.register_iterator::<SequenceCursor>();
    engine.register_iterator::<MappingCursor>();
    engine.register_iterator::<StreamCursor>();

    // yaml::edit(text) -> YamlStream
    module.set_native_fn("edit", |text: ImmutableString| edit(&text));
}
