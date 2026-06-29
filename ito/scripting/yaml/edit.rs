//! Format-preserving YAML editing for the Lua surface.
//!
//! `yaml.edit(text)` parses the source into a shared, mutable concrete
//! syntax tree (`yamlito::SyntaxTree`) and hands back a [`YamlCursor`]
//! over its root (a `Stream` variant). The `:doc()` method steps to the
//! first document's root cursor (erroring on an empty stream); further
//! navigation (`stream[0]`, `m[key]`, `s[i]` — Lua indexing via the
//! `__index` metamethod) resolves a step against the live tree and fails
//! immediately with a clear error if the path doesn't exist. Edits
//! (`:write`, `:set_comment`, `:rename_key`, `:make_single_quoted`, …)
//! mutate the tree in place. `:to_string()` renders the (possibly
//! mutated) source, layout and comments intact — unlike the lossy
//! `yaml.parse`/`yaml.to_string` round-trip.
//!
//! ## One unified cursor type
//!
//! The Lua surface represents every kind of node with a single
//! [`YamlCursor`] enum. Lua's UserData method table is a flat namespace,
//! so every method is registered once and dispatches on the receiver's
//! variant internally, returning a clear runtime error when the receiver
//! is the wrong kind (e.g. calling `:read()` on a mapping). The `Null`
//! variant behaves as a null object: navigation methods return empty
//! collections / empty strings.
//!
//! `:kind()` names the variant (`"null"`, `"mapping"`, `"sequence"`,
//! `"scalar"`, `"stream"`), and one `:is_*()` predicate per kind reads
//! the same value, so a script can pick whichever reads better at the
//! call site without the two ever disagreeing.
//!
//! ## Iteration
//!
//! There is no implicit `for x in cursor`; iteration goes through
//! explicit helper methods that return plain arrays, iterable with
//! ordinary `ipairs`:
//!
//! - `mapping:entries()` -> array of `YamlEntry` (each has `:key()`,
//!   `:value()` cursor, `:key_node()` cursor).
//! - `mapping:values()` -> array of value cursors (already existed).
//! - `sequence:values()` — the sequence has no `values()`; iterate with
//!   `for i = 0, s:len() - 1 do local e = s[i] ... end`, or use the typed
//!   selectors `:mappings()` / `:sequences()` / `:scalars()`.
//! - `stream:docs()` -> array of document-root cursors.
//!
//! All cursors share one `Rc<RefCell<SyntaxTree>>`, so mutations made
//! through one are visible through the others.

use std::cell::RefCell;
use std::rc::Rc;

use mlua::{Lua, LuaSerdeExt, MetaMethod, MultiValue, UserData, UserDataMethods, Value};

use yamlito::SyntaxTree;
use yamlito::Yaml;
use yamlito::ast::{AstNode, Mapping, Node, Scalar, Value as YamlValue};
use yamlito::edit::{
    GreenElement, TreeEdit, apply_edit, insert_map_entry, insert_seq_item, mk_node, mk_token,
    push_seq_item, remove_map_entry, remove_seq_item,
};
use yamlito::lexer::{as_plain_scalar, as_single_quoted};
use yamlito::syntax::{SyntaxKind, SyntaxNode};

use super::comment;

type Result<T> = mlua::Result<T>;

fn err(msg: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(msg.to_string())
}

/// Convert a script-facing 1-based index to the 0-based position the
/// underlying `yamlito::edit` primitives expect (matching Lua's own
/// `string.sub`/`table.insert` convention — index `1` is the first
/// element). Rejects `0` and negative indices; there is no "from the
/// end" meaning for these positional edits.
fn zero_based(index: i64) -> Result<usize> {
    if index < 1 {
        return Err(err(format!("index must be >= 1, got {index}")));
    }
    Ok((index - 1) as usize)
}

/// Shared handle to the parsed tree. Cloning is cheap.
type TreeHandle = Rc<RefCell<SyntaxTree>>;

/// A single, unified cursor over a live YAML syntax tree. The variant
/// records what kind of node the cursor points at; methods dispatch on
/// the variant and error when called on the wrong kind.
#[derive(Clone)]
pub enum YamlCursor {
    /// Null object returned for an empty document. Navigation methods
    /// return empty collections / strings.
    Null,
    /// Points at a block or flow mapping.
    Mapping { tree: TreeHandle, node: SyntaxNode },
    /// Points at a block or flow sequence.
    Sequence { tree: TreeHandle, node: SyntaxNode },
    /// Points at a scalar (any style) or alias.
    Scalar { tree: TreeHandle, node: SyntaxNode },
    /// Points at the whole stream; indexable by document number.
    Stream { tree: TreeHandle, node: SyntaxNode },
}

impl YamlCursor {
    fn kind(&self) -> &'static str {
        match self {
            YamlCursor::Null => "null",
            YamlCursor::Mapping { .. } => "mapping",
            YamlCursor::Sequence { .. } => "sequence",
            YamlCursor::Scalar { .. } => "scalar",
            YamlCursor::Stream { .. } => "stream",
        }
    }

    fn wrong_kind(&self, method: &str, expected: &str) -> mlua::Error {
        err(format!("{method}: not a {expected} (got {})", self.kind()))
    }
}

/// Yielded by mapping iteration (`mapping:entries()`). Holds the decoded
/// key and a value cursor. Exposed to scripts as `YamlEntry` with
/// `:key()`, `:value()`, and `:key_node()` accessors.
#[derive(Clone)]
pub struct MapEntry {
    key: String,
    value: YamlCursor,
    key_node: SyntaxNode,
    tree: TreeHandle,
}

impl MapEntry {
    /// Cursor (scalar) at the entry's key node.
    fn key_cursor(&self) -> YamlCursor {
        cursor_for(Rc::clone(&self.tree), self.key_node.clone())
    }
}

impl UserData for MapEntry {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("key", |_, e, ()| Ok(e.key.clone()));
        methods.add_method("value", |_, e, ()| Ok(e.value.clone()));
        methods.add_method("key_node", |_, e, ()| Ok(e.key_cursor()));
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

/// Entry position of `key` in the mapping (counting all entries in
/// source order), or `None` if absent.
fn mapping_key_index(map_node: &SyntaxNode, key: &str) -> Option<usize> {
    let node = Node::cast(map_node.clone())?;
    node.as_mapping()?.pairs().position(
        |(k, _)| matches!(&k, Node::Scalar(s) if s.decoded().ok().as_deref() == Some(key)),
    )
}

/// Wrap a `Node`-resolved SyntaxNode into the right cursor variant.
fn cursor_for(tree: TreeHandle, node: SyntaxNode) -> YamlCursor {
    match node.kind() {
        SyntaxKind::BLOCK_MAPPING | SyntaxKind::FLOW_MAPPING => YamlCursor::Mapping { tree, node },
        SyntaxKind::BLOCK_SEQUENCE | SyntaxKind::FLOW_SEQUENCE => {
            YamlCursor::Sequence { tree, node }
        }
        SyntaxKind::SCALAR => YamlCursor::Scalar { tree, node },
        // ALIAS: expose as a scalar cursor for now — read() will emit
        // the alias text. Revisit if users want to follow aliases.
        SyntaxKind::ALIAS => YamlCursor::Scalar { tree, node },
        // Fall back to a Null cursor for unexpected kinds rather than a
        // stringly-typed value — there is no polymorphic value slot in
        // the typed Lua surface. Navigation to such a node is not expected.
        _ => YamlCursor::Null,
    }
}

/// Look up a mapping entry by key and produce a cursor on its value.
fn map_step_key(tree: &TreeHandle, node: &SyntaxNode, key: &str) -> Result<YamlCursor> {
    let value_node =
        resolve_map_value(node, key).ok_or_else(|| err(format!("key not found: {key}")))?;
    Ok(cursor_for(Rc::clone(tree), value_node))
}

/// Look up a sequence element by 1-based index (`s[1]` is the first
/// element, matching Lua's own indexing convention).
fn seq_step_index(tree: &TreeHandle, node: &SyntaxNode, i: i64) -> Result<YamlCursor> {
    let idx = zero_based(i)?;
    let elem =
        resolve_seq_index(node, idx).ok_or_else(|| err(format!("index out of range: {i}")))?;
    Ok(cursor_for(Rc::clone(tree), elem))
}

fn resolve_map_value(map_node: &SyntaxNode, key: &str) -> Option<SyntaxNode> {
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

/// Convert a decoded scalar `Value` into a Lua value.
fn value_to_lua(lua: &Lua, v: &YamlValue) -> Result<Value> {
    Ok(match v {
        YamlValue::Null => Value::Nil,
        YamlValue::Bool(b) => Value::Boolean(*b),
        YamlValue::Int(i) => Value::Integer(*i),
        YamlValue::Float(f) => Value::Number(*f),
        YamlValue::String(s) => Value::String(lua.create_string(s)?),
    })
}

/// Encode a Lua value as YAML scalar text (nil/bool/int/float/string).
/// Returns `None` for non-scalar values (tables, etc.).
fn lua_to_yaml_scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::Nil => Some("null".to_string()),
        Value::Boolean(b) => Some(b.to_string()),
        Value::Integer(i) => Some(i.to_string()),
        Value::Number(f) => Some(f.to_string()),
        Value::String(s) => Some(yaml_quote_if_needed(&s.to_string_lossy())),
        _ => None,
    }
}

/// Convert a Lua value into a `yamlito::Yaml` value model, ready to
/// splice into a collection. Scalars, arrays, and object maps are all
/// supported via the serde bridge that `yaml.to_string` uses.
fn lua_to_yaml(lua: &Lua, value: &Value) -> Result<Yaml> {
    lua.from_value(value.clone())
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

// ---------------------------------------------------------------------------
// Per-variant operation helpers
// ---------------------------------------------------------------------------

fn mapping_len(node: &SyntaxNode) -> i64 {
    Node::cast(node.clone())
        .as_ref()
        .and_then(Node::as_mapping)
        .map_or(0, |m| m.len() as i64)
}

fn mapping_keys(node: &SyntaxNode) -> Vec<String> {
    Node::cast(node.clone())
        .as_ref()
        .and_then(Node::as_mapping)
        .map(|m| m.keys().collect())
        .unwrap_or_default()
}

fn mapping_values(tree: &TreeHandle, node: &SyntaxNode) -> Vec<YamlCursor> {
    let nodes: Vec<SyntaxNode> = Node::cast(node.clone())
        .as_ref()
        .and_then(Node::as_mapping)
        .map(|m| m.values().map(|n| node_syntax(&n)).collect())
        .unwrap_or_default();
    nodes
        .into_iter()
        .map(|n| cursor_for(Rc::clone(tree), n))
        .collect()
}

fn mapping_entries(tree: &TreeHandle, node: &SyntaxNode) -> Vec<MapEntry> {
    let pairs: Vec<(String, SyntaxNode, SyntaxNode)> = Node::cast(node.clone())
        .as_ref()
        .and_then(Node::as_mapping)
        .map(collect_map_pairs)
        .unwrap_or_default();
    pairs
        .into_iter()
        .map(|(k, kn, v)| MapEntry {
            key: k,
            value: cursor_for(Rc::clone(tree), v),
            key_node: kn,
            tree: Rc::clone(tree),
        })
        .collect()
}

fn mapping_contains(node: &SyntaxNode, key: &str) -> bool {
    Node::cast(node.clone())
        .as_ref()
        .and_then(Node::as_mapping)
        .is_some_and(|m| m.get(key).is_some())
}

/// Shared add path for `append`/`insert_at`: reject a duplicate key,
/// then splice a fresh entry at `index` (clamped). Returns the fresh
/// mapping node.
fn map_add(
    lua: &Lua,
    tree: &TreeHandle,
    node: &SyntaxNode,
    index: usize,
    key: &str,
    value: &Value,
) -> Result<SyntaxNode> {
    if mapping_key_index(node, key).is_some() {
        return Err(err(format!("duplicate key: {key}")));
    }
    let item = lua_to_yaml(lua, value)?;
    let mut tree = tree.borrow_mut();
    Ok(insert_map_entry(&mut tree, node, index, key, item))
}

fn mapping_rename_key(
    tree: &TreeHandle,
    node: &SyntaxNode,
    old_key: &str,
    new_key: &str,
) -> Result<bool> {
    let Some(key_syntax) = mapping_key_node(node, old_key) else {
        return Ok(false);
    };

    let scalar = yamlito::ast::Scalar::cast(key_syntax.clone())
        .ok_or_else(|| err("key is not a scalar node"))?;
    let tok = scalar
        .value_token()
        .ok_or_else(|| err("key scalar has no value token"))?;

    let text = yaml_quote_if_needed(new_key);
    let (token_kind, token_text) = if text.starts_with('\'') && text.ends_with('\'') {
        (SyntaxKind::SINGLE_QUOTED_SCALAR, text)
    } else {
        (SyntaxKind::PLAIN_SCALAR, text)
    };

    let mut tree = tree.borrow_mut();
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

fn seq_len(node: &SyntaxNode) -> i64 {
    Node::cast(node.clone())
        .as_ref()
        .and_then(Node::as_sequence)
        .map_or(0, |s| s.len() as i64)
}

fn seq_elements_of_kind(
    tree: &TreeHandle,
    node: &SyntaxNode,
    predicate: impl Fn(SyntaxKind) -> bool,
) -> Vec<YamlCursor> {
    let children: Vec<SyntaxNode> = Node::cast(node.clone())
        .as_ref()
        .and_then(Node::as_sequence)
        .map(|s| s.iter().map(|n| node_syntax(&n)).collect())
        .unwrap_or_default();
    children
        .into_iter()
        .filter(|n| predicate(n.kind()))
        .map(|n| cursor_for(Rc::clone(tree), n))
        .collect()
}

fn scalar_read(lua: &Lua, node: &SyntaxNode) -> Result<Value> {
    let scalar = Scalar::cast(node.clone()).ok_or_else(|| err("not a scalar"))?;
    let v = scalar
        .to_value()
        .map_err(|e| err(format!("decode error: {e:?}")))?;
    value_to_lua(lua, &v)
}

fn scalar_write(tree: &TreeHandle, node: &SyntaxNode, value: &Value) -> Result<()> {
    let text = lua_to_yaml_scalar_text(value)
        .ok_or_else(|| err("cannot write this value as a YAML scalar"))?;
    let (token_kind, token_text) = if text.starts_with('\'') && text.ends_with('\'') {
        (SyntaxKind::SINGLE_QUOTED_SCALAR, text.clone())
    } else {
        (SyntaxKind::PLAIN_SCALAR, text.clone())
    };
    let scalar = Scalar::cast(node.clone()).ok_or_else(|| err("not a scalar"))?;
    let Some(current_tok) = scalar.value_token() else {
        return Err(err("scalar has no value token"));
    };

    let mut tree = tree.borrow_mut();
    if current_tok.kind() == token_kind {
        let new = mk_token(token_kind, &token_text);
        apply_edit(
            &mut tree,
            TreeEdit::ReplaceToken {
                target: current_tok,
                new,
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
                target: node.clone(),
                new: new_node,
            },
        );
    }
    Ok(())
}

fn scalar_try_make_unquoted(tree: &TreeHandle, node: &SyntaxNode) -> bool {
    let scalar = match Scalar::cast(node.clone()) {
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
    let mut tree = tree.borrow_mut();
    apply_edit(
        &mut tree,
        TreeEdit::ReplaceNode {
            target: node.clone(),
            new,
        },
    );
    true
}

fn scalar_try_make_single_quoted(tree: &TreeHandle, node: &SyntaxNode) -> bool {
    let scalar = match Scalar::cast(node.clone()) {
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
    let mut tree = tree.borrow_mut();
    apply_edit(
        &mut tree,
        TreeEdit::ReplaceNode {
            target: node.clone(),
            new,
        },
    );
    true
}

fn stream_docs(tree: &TreeHandle, node: &SyntaxNode) -> Vec<YamlCursor> {
    let stream = yamlito::ast::Stream::cast(node.clone());
    match stream {
        Some(st) => st
            .documents()
            .filter_map(|d| d.root_node())
            .map(|n| cursor_for(Rc::clone(tree), node_syntax(&n)))
            .collect(),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// UserData: the unified cursor
// ---------------------------------------------------------------------------

impl UserData for YamlCursor {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // --- kind: the cursor's node kind as a string, and one is_*()
        //     predicate per kind -- both read the same `kind()` helper, so
        //     `c:kind() == "mapping"` and `c:is_mapping()` cannot disagree. ---
        methods.add_method("kind", |_, this, ()| Ok(this.kind()));
        methods.add_method("is_null", |_, this, ()| Ok(this.kind() == "null"));
        methods.add_method("is_mapping", |_, this, ()| Ok(this.kind() == "mapping"));
        methods.add_method("is_sequence", |_, this, ()| Ok(this.kind() == "sequence"));
        methods.add_method("is_scalar", |_, this, ()| Ok(this.kind() == "scalar"));
        methods.add_method("is_stream", |_, this, ()| Ok(this.kind() == "stream"));

        // --- len: mapping (entry count), sequence (item count), stream
        //     (document count), null (0). ---
        methods.add_method("len", |_, this, ()| {
            Ok(match this {
                YamlCursor::Null => 0i64,
                YamlCursor::Mapping { node, .. } => mapping_len(node),
                YamlCursor::Sequence { node, .. } => seq_len(node),
                YamlCursor::Stream { node, .. } => yamlito::ast::Stream::cast(node.clone())
                    .map(|st| st.len() as i64)
                    .unwrap_or(0),
                YamlCursor::Scalar { .. } => return Err(this.wrong_kind("len", "collection")),
            })
        });

        // --- to_string: raw source text of the node (stream renders the
        //     whole, possibly-mutated tree). Null -> "". ---
        methods.add_method("to_string", |_, this, ()| {
            Ok(match this {
                YamlCursor::Null => String::new(),
                YamlCursor::Mapping { node, .. }
                | YamlCursor::Sequence { node, .. }
                | YamlCursor::Scalar { node, .. } => node.text().to_string(),
                YamlCursor::Stream { tree, .. } => tree.borrow().emit(),
            })
        });

        // --- keys: mapping only (null -> {}). ---
        methods.add_method("keys", |_, this, ()| {
            Ok(match this {
                YamlCursor::Null => Vec::new(),
                YamlCursor::Mapping { node, .. } => mapping_keys(node),
                _ => return Err(this.wrong_kind("keys", "mapping")),
            })
        });

        // --- values: mapping value cursors in source order (null -> {}). ---
        methods.add_method("values", |_, this, ()| {
            Ok(match this {
                YamlCursor::Null => Vec::new(),
                YamlCursor::Mapping { tree, node } => mapping_values(tree, node),
                _ => return Err(this.wrong_kind("values", "mapping")),
            })
        });

        // --- entries: mapping iteration helper -> array of YamlEntry
        //     (null -> {}). ---
        methods.add_method("entries", |_, this, ()| {
            Ok(match this {
                YamlCursor::Null => Vec::new(),
                YamlCursor::Mapping { tree, node } => mapping_entries(tree, node),
                _ => return Err(this.wrong_kind("entries", "mapping")),
            })
        });

        // --- contains: mapping only (null -> false). ---
        methods.add_method("contains", |_, this, key: String| {
            Ok(match this {
                YamlCursor::Null => false,
                YamlCursor::Mapping { node, .. } => mapping_contains(node, &key),
                _ => return Err(this.wrong_kind("contains", "mapping")),
            })
        });

        // --- get: explicit alternative to bracket indexing.
        //     mapping[key] / sequence[i] / stream[i]. ---
        methods.add_method("get", |_, this, key: Value| index_step(this, key));

        // --- mappings / sequences / scalars: sequence element selectors
        //     (null -> {}). ---
        methods.add_method("mappings", |_, this, ()| {
            Ok(match this {
                YamlCursor::Null => Vec::new(),
                YamlCursor::Sequence { tree, node } => seq_elements_of_kind(tree, node, |k| {
                    matches!(k, SyntaxKind::BLOCK_MAPPING | SyntaxKind::FLOW_MAPPING)
                }),
                _ => return Err(this.wrong_kind("mappings", "sequence")),
            })
        });
        methods.add_method("sequences", |_, this, ()| {
            Ok(match this {
                YamlCursor::Null => Vec::new(),
                YamlCursor::Sequence { tree, node } => seq_elements_of_kind(tree, node, |k| {
                    matches!(k, SyntaxKind::BLOCK_SEQUENCE | SyntaxKind::FLOW_SEQUENCE)
                }),
                _ => return Err(this.wrong_kind("sequences", "sequence")),
            })
        });
        methods.add_method("scalars", |_, this, ()| {
            Ok(match this {
                YamlCursor::Null => Vec::new(),
                YamlCursor::Sequence { tree, node } => seq_elements_of_kind(tree, node, |k| {
                    matches!(k, SyntaxKind::SCALAR | SyntaxKind::ALIAS)
                }),
                _ => return Err(this.wrong_kind("scalars", "sequence")),
            })
        });

        // --- doc / docs: stream navigation. ---
        methods.add_method("doc", |_, this, ()| match this {
            YamlCursor::Stream { tree, node } => {
                let root = yamlito::ast::Stream::cast(node.clone())
                    .and_then(|st| st.documents().next())
                    .and_then(|d| d.root_node())
                    .ok_or_else(|| err("stream has no document"))?;
                Ok(cursor_for(Rc::clone(tree), node_syntax(&root)))
            }
            _ => Err(this.wrong_kind("doc", "stream")),
        });
        methods.add_method("docs", |_, this, ()| {
            Ok(match this {
                YamlCursor::Null => Vec::new(),
                YamlCursor::Stream { tree, node } => stream_docs(tree, node),
                _ => return Err(this.wrong_kind("docs", "stream")),
            })
        });

        // --- read / write: scalar only. ---
        methods.add_method("read", |lua, this, ()| match this {
            YamlCursor::Scalar { node, .. } => scalar_read(lua, node),
            _ => Err(this.wrong_kind("read", "scalar")),
        });
        methods.add_method("write", |_, this, value: Value| match this {
            YamlCursor::Scalar { tree, node } => scalar_write(tree, node, &value),
            _ => Err(this.wrong_kind("write", "scalar")),
        });

        // --- scalar style helpers. ---
        methods.add_method("try_make_unquoted", |_, this, ()| match this {
            YamlCursor::Scalar { tree, node } => Ok(scalar_try_make_unquoted(tree, node)),
            _ => Err(this.wrong_kind("try_make_unquoted", "scalar")),
        });
        methods.add_method("make_unquoted", |_, this, ()| match this {
            YamlCursor::Scalar { tree, node } => {
                if scalar_try_make_unquoted(tree, node) {
                    Ok(())
                } else {
                    Err(err("scalar cannot be represented as an unquoted value"))
                }
            }
            _ => Err(this.wrong_kind("make_unquoted", "scalar")),
        });
        methods.add_method("try_make_single_quoted", |_, this, ()| match this {
            YamlCursor::Scalar { tree, node } => Ok(scalar_try_make_single_quoted(tree, node)),
            _ => Err(this.wrong_kind("try_make_single_quoted", "scalar")),
        });
        methods.add_method("make_single_quoted", |_, this, ()| match this {
            YamlCursor::Scalar { tree, node } => {
                if scalar_try_make_single_quoted(tree, node) {
                    Ok(())
                } else {
                    Err(err("scalar cannot be represented as a single-quoted value"))
                }
            }
            _ => Err(this.wrong_kind("make_single_quoted", "scalar")),
        });

        // --- comment accessors: mapping / sequence / scalar / stream /
        //     null. Leading comment. ---
        methods.add_method("comment", |_, this, ()| {
            Ok(match this {
                YamlCursor::Null => String::new(),
                YamlCursor::Mapping { node, .. }
                | YamlCursor::Sequence { node, .. }
                | YamlCursor::Scalar { node, .. } => comment::read_leading(node),
                YamlCursor::Stream { node, .. } => yamlito::ast::Stream::cast(node.clone())
                    .map(|st| st.leading_comment())
                    .unwrap_or_default(),
            })
        });
        methods.add_method("set_comment", |_, this, text: String| match this {
            YamlCursor::Mapping { tree, node }
            | YamlCursor::Sequence { tree, node }
            | YamlCursor::Scalar { tree, node } => {
                let mut tree = tree.borrow_mut();
                comment::write_leading(&mut tree, node, &text).map_err(err)
            }
            YamlCursor::Stream { tree, node } => {
                let stream =
                    yamlito::ast::Stream::cast(node.clone()).ok_or_else(|| err("not a stream"))?;
                let mut tree = tree.borrow_mut();
                stream
                    .set_leading_comment(&mut tree, Some(&text))
                    .map_err(err)
            }
            YamlCursor::Null => Err(this.wrong_kind("set_comment", "node")),
        });
        methods.add_method("clear_comment", |_, this, ()| match this {
            YamlCursor::Mapping { tree, node }
            | YamlCursor::Sequence { tree, node }
            | YamlCursor::Scalar { tree, node } => {
                let mut tree = tree.borrow_mut();
                comment::clear_leading(&mut tree, node);
                Ok(())
            }
            YamlCursor::Stream { tree, node } => {
                let stream =
                    yamlito::ast::Stream::cast(node.clone()).ok_or_else(|| err("not a stream"))?;
                let mut tree = tree.borrow_mut();
                stream.set_leading_comment(&mut tree, None).map_err(err)
            }
            YamlCursor::Null => Ok(()),
        });

        // --- trailing comment accessors: mapping / sequence / scalar /
        //     null (stream has no trailing-comment concept). ---
        methods.add_method("trailing_comment", |_, this, ()| {
            Ok(match this {
                YamlCursor::Null => String::new(),
                YamlCursor::Mapping { node, .. }
                | YamlCursor::Sequence { node, .. }
                | YamlCursor::Scalar { node, .. } => comment::read_trailing(node),
                YamlCursor::Stream { .. } => {
                    return Err(this.wrong_kind("trailing_comment", "entry-bearing node"));
                }
            })
        });
        methods.add_method("set_trailing_comment", |_, this, text: String| match this {
            YamlCursor::Mapping { tree, node }
            | YamlCursor::Sequence { tree, node }
            | YamlCursor::Scalar { tree, node } => {
                let mut tree = tree.borrow_mut();
                comment::write_trailing(&mut tree, node, &text).map_err(err)
            }
            _ => Err(this.wrong_kind("set_trailing_comment", "entry-bearing node")),
        });
        methods.add_method("clear_trailing_comment", |_, this, ()| match this {
            YamlCursor::Mapping { tree, node }
            | YamlCursor::Sequence { tree, node }
            | YamlCursor::Scalar { tree, node } => {
                let mut tree = tree.borrow_mut();
                comment::clear_trailing(&mut tree, node);
                Ok(())
            }
            _ => Err(this.wrong_kind("clear_trailing_comment", "entry-bearing node")),
        });

        // --- rename_key: mapping only. ---
        methods.add_method(
            "rename_key",
            |_, this, (old_key, new_key): (String, String)| match this {
                YamlCursor::Mapping { tree, node } => {
                    mapping_rename_key(tree, node, &old_key, &new_key)
                }
                _ => Err(this.wrong_kind("rename_key", "mapping")),
            },
        );

        // --- append: mapping (key, value) / sequence (value). Refreshes
        //     the cursor's node in place. ---
        methods.add_method_mut("append", |lua, this, mut args: MultiValue| match this {
            YamlCursor::Mapping { tree, node } => {
                let key: String = pop_arg(lua, &mut args, "append: key")?;
                let value: Value = args.pop_front().unwrap_or(Value::Nil);
                let fresh = map_add(lua, tree, node, usize::MAX, &key, &value)?;
                *node = fresh;
                Ok(())
            }
            YamlCursor::Sequence { tree, node } => {
                let value: Value = args.pop_front().unwrap_or(Value::Nil);
                let item = lua_to_yaml(lua, &value)?;
                let fresh = {
                    let mut tree = tree.borrow_mut();
                    push_seq_item(&mut tree, node, item)
                };
                *node = fresh;
                Ok(())
            }
            _ => Err(this.wrong_kind("append", "mapping or sequence")),
        });

        // --- insert_at: mapping (index, key, value) / sequence (index,
        //     value). `index` is 1-based, matching Lua's own indexing
        //     convention; out-of-range (past the end) clamps to append. ---
        methods.add_method_mut("insert_at", |lua, this, mut args: MultiValue| match this {
            YamlCursor::Mapping { tree, node } => {
                let index: i64 = pop_arg(lua, &mut args, "insert_at: index")?;
                let pos = zero_based(index)?;
                let key: String = pop_arg(lua, &mut args, "insert_at: key")?;
                let value: Value = args.pop_front().unwrap_or(Value::Nil);
                let fresh = map_add(lua, tree, node, pos, &key, &value)?;
                *node = fresh;
                Ok(())
            }
            YamlCursor::Sequence { tree, node } => {
                let index: i64 = pop_arg(lua, &mut args, "insert_at: index")?;
                let pos = zero_based(index)?;
                let value: Value = args.pop_front().unwrap_or(Value::Nil);
                let item = lua_to_yaml(lua, &value)?;
                let fresh = {
                    let mut tree = tree.borrow_mut();
                    insert_seq_item(&mut tree, node, pos, item)
                };
                *node = fresh;
                Ok(())
            }
            _ => Err(this.wrong_kind("insert_at", "mapping or sequence")),
        });

        // --- append_or_update: mapping only. ---
        methods.add_method_mut(
            "append_or_update",
            |lua, this, (key, value): (String, Value)| match this {
                YamlCursor::Mapping { tree, node } => {
                    let item = lua_to_yaml(lua, &value)?;
                    let existing = mapping_key_index(node, &key);
                    let fresh = {
                        let mut tree = tree.borrow_mut();
                        match existing {
                            Some(i) => {
                                let n = insert_map_entry(&mut tree, node, i, &key, item);
                                remove_map_entry(&mut tree, &n, i + 1)
                            }
                            None => insert_map_entry(&mut tree, node, usize::MAX, &key, item),
                        }
                    };
                    *node = fresh;
                    Ok(())
                }
                _ => Err(this.wrong_kind("append_or_update", "mapping")),
            },
        );

        // --- remove: mapping (by key) -> bool. ---
        methods.add_method_mut("remove", |_, this, key: String| match this {
            YamlCursor::Mapping { tree, node } => {
                let Some(i) = mapping_key_index(node, &key) else {
                    return Ok(false);
                };
                let fresh = {
                    let mut tree = tree.borrow_mut();
                    remove_map_entry(&mut tree, node, i)
                };
                *node = fresh;
                Ok(true)
            }
            _ => Err(this.wrong_kind("remove", "mapping")),
        });

        // --- remove_at: mapping / sequence (by 1-based index, matching
        //     Lua's own indexing convention; out-of-range is a no-op). ---
        methods.add_method_mut("remove_at", |_, this, index: i64| {
            let pos = zero_based(index)?;
            match this {
                YamlCursor::Mapping { tree, node } => {
                    let fresh = {
                        let mut tree = tree.borrow_mut();
                        remove_map_entry(&mut tree, node, pos)
                    };
                    *node = fresh;
                    Ok(())
                }
                YamlCursor::Sequence { tree, node } => {
                    let fresh = {
                        let mut tree = tree.borrow_mut();
                        remove_seq_item(&mut tree, node, pos)
                    };
                    *node = fresh;
                    Ok(())
                }
                _ => Err(this.wrong_kind("remove_at", "mapping or sequence")),
            }
        });

        // --- __index metamethod: bracket indexing `m[key]`, `s[i]`,
        //     `stream[i]`. Registered methods take precedence (mlua uses
        //     this meta-method only as a fall-back), so `m["append"]`
        //     still resolves the method. Only bracket access with a key
        //     that is not a method name lands here. ---
        methods.add_meta_method(MetaMethod::Index, |_, this, key: Value| {
            index_step(this, key)
        });
    }
}

/// Pull the next positional argument from a `MultiValue`, converting it
/// to `T` with a contextual error message.
fn pop_arg<T: mlua::FromLua>(lua: &Lua, args: &mut MultiValue, ctx: &str) -> Result<T> {
    let v = args
        .pop_front()
        .ok_or_else(|| err(format!("{ctx}: missing argument")))?;
    T::from_lua(v, lua)
}

/// Bracket-index / `:get()` step: `mapping[string]`, `sequence[int]`,
/// `stream[int]`. Null indexes to Null. Scalars are not indexable.
fn index_step(this: &YamlCursor, key: Value) -> Result<YamlCursor> {
    match this {
        YamlCursor::Null => Ok(YamlCursor::Null),
        YamlCursor::Mapping { tree, node } => {
            let key = match key {
                Value::String(s) => s.to_string_lossy(),
                other => {
                    return Err(err(format!(
                        "mapping index must be a string, got {}",
                        other.type_name()
                    )));
                }
            };
            map_step_key(tree, node, &key)
        }
        YamlCursor::Sequence { tree, node } => {
            let i = as_index(&key, "sequence")?;
            seq_step_index(tree, node, i)
        }
        YamlCursor::Stream { tree, node } => {
            let i = as_index(&key, "stream")?;
            let idx = zero_based(i)?;
            let stream =
                yamlito::ast::Stream::cast(node.clone()).ok_or_else(|| err("not a stream"))?;
            let doc = stream
                .documents()
                .nth(idx)
                .ok_or_else(|| err(format!("document index out of range: {i}")))?;
            let root = doc
                .root_node()
                .ok_or_else(|| err(format!("document {i} is empty")))?;
            Ok(cursor_for(Rc::clone(tree), node_syntax(&root)))
        }
        // Lua/Luau give `x:foo()` and `x["foo"]` the same __index dispatch,
        // so a call to a method that doesn't exist on this cursor kind (a
        // typo, or one that only exists on another kind) lands here too --
        // say so rather than implying deliberate indexing was attempted.
        YamlCursor::Scalar { .. } => Err(match key {
            Value::String(s) => err(format!(
                "'{}' is not a method on a scalar cursor (and scalars can't be indexed)",
                s.to_string_lossy()
            )),
            _ => err("cannot index a scalar cursor"),
        }),
    }
}

fn as_index(key: &Value, what: &str) -> Result<i64> {
    match key {
        Value::Integer(i) => Ok(*i),
        Value::Number(f) if f.fract() == 0.0 => Ok(*f as i64),
        other => Err(err(format!(
            "{what} index must be an integer, got {}",
            other.type_name()
        ))),
    }
}

/// Parse `text` into a shared mutable tree and return a `Stream` cursor
/// over its root. Parse failures surface as Lua runtime errors.
pub fn edit(text: &str) -> Result<YamlCursor> {
    let tree = yamlito::parse(text).map_err(|e| err(format!("yaml parse error: {e}")))?;
    let tree: TreeHandle = Rc::new(RefCell::new(tree));
    let node = tree.borrow().root().clone();
    Ok(YamlCursor::Stream { tree, node })
}

/// Register the `yaml.edit` entry point on the `yaml` module table.
/// The unified [`YamlCursor`] / [`MapEntry`] userdata carry their own
/// metatables, so no separate type registration is needed.
pub fn register(lua: &Lua, module: &mlua::Table) -> Result<()> {
    module.set("edit", lua.create_function(|_, text: String| edit(&text))?)?;
    Ok(())
}
