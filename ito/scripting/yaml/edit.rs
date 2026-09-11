//! Format-preserving YAML editing for the Lua surface.
//!
//! `yaml.edit(text)` parses the source into a shared, mutable concrete
//! syntax tree (`yamlito::SyntaxTree`) and hands back a [`YamlCursor`]
//! over its root. Edits (`:write`, `:set_comment`, `:rename_key`,
//! `:single_quote`, …) mutate the tree in place. `:to_string()`
//! renders the (possibly mutated) source, layout and comments intact —
//! unlike the lossy `yaml.parse`/`yaml.to_string` round-trip.
//!
//! ## A cursor is a path, not a node
//!
//! A [`YamlCursor`] holds the shared tree plus a `Vec<Segment>` — inert,
//! owned coordinates. Traversal (`stream[i]`, `m[key]`, `s[i]`, `:doc()`)
//! only *appends a segment*: it never touches the tree, so it never
//! fails. Every read and every write calls [`YamlCursor::resolve`], which
//! walks the path from the tree's **current** root.
//!
//! This is the same model as `hcl.edit`, and it exists for the same
//! reason. yamlito's tree is persistent: an edit rebuilds the ancestor
//! spine to a fresh root, so a cached `SyntaxNode` taken beforehand
//! points into the discarded one. Caching nodes meant a cursor could
//! *read* a phantom value from the old tree while *writing* to whatever
//! now sat at its old index — and a path that no longer resolved aborted
//! the process. Resolving on every call makes both impossible.
//!
//! The trade is that a path is a **coordinate, not an identity**: `s[3]`
//! means "whatever is at index 3 *now*". Key-addressed cursors
//! (`m["port"]`) survive sibling insertion and removal; index-addressed
//! ones shift with it. That is why `:entries()` and `:values()` hand back
//! key-addressed cursors.
//!
//! Because traversal cannot fail, a missing key surfaces at the next
//! operation rather than at the line that named it; the error carries the
//! path it got through (`key not found: c (at doc(1).b)`). `:exists()`
//! answers the question directly, and the `try_*` methods swallow it.
//!
//! ## One unified cursor type
//!
//! One Luau userdata represents every kind of node. Lua's UserData method
//! table is a flat namespace, so every method is registered once and
//! checks the *resolved* node's kind, returning a clear runtime error
//! when it is the wrong one (e.g. calling `:read()` on a mapping).
//!
//! `:kind()` names that kind (`"stream"`, `"mapping"`, `"sequence"`,
//! `"scalar"`, `"alias"`), and one `:is_*()` predicate per kind reads the
//! same value, so a script can pick whichever reads better at the call
//! site without the two ever disagreeing. Both resolve, so both raise on
//! a path that isn't there — `:exists()` and `:try_kind()` are the
//! non-raising forms.
//!
//! ## Iteration
//!
//! There is no implicit `for x in cursor`; iteration goes through
//! explicit helper methods that return plain arrays, iterable with
//! ordinary `ipairs`:
//!
//! - `mapping:entries()` -> array of `YamlEntry` (each has `:key()`,
//!   `:value()` cursor, `:key_node()` cursor).
//! - `mapping:values()` -> array of value cursors.
//! - `sequence:values()` — the sequence has no `values()`; iterate with
//!   `for i = 1, s:len() do local e = s[i] ... end`, or use the typed
//!   selectors `:mappings()` / `:sequences()` / `:scalars()`.
//! - `stream:docs()` -> array of document-root cursors.
//!
//! All cursors share one `Rc<RefCell<SyntaxTree>>`, so mutations made
//! through one are visible through the others.
//!
//! ## Scalar quoting
//!
//! One verb-named method per style (`unquote`, `single_quote`,
//! `double_quote`) plus `auto_quote` for the narrowest style that
//! round-trips, and `quote_style()` to read the current one. Each
//! converts by **decoding the scalar and re-encoding it**, not by
//! shuffling raw text, which is what makes every pair of styles
//! reachable (`'it''s'` → `"it's"`) rather than only the pairs whose
//! text happens to transfer.
//!
//! Each returns a `bool` for whether the conversion happened, rather
//! than raising on an unrepresentable value — there is no other natural
//! return value for a restyle, and there is no `try_` twin because the
//! non-raising bool return already *is* the "try" form. `double_quote`
//! and `auto_quote` always return `true` for a representable value:
//! double quotes have an escape for every character. Each still raises
//! on a non-scalar receiver, since that is a usage error rather than a
//! "can't represent".
//!
//! The policy comes from `yamlito::quoting`, **not** `yamlito::lexer`:
//! the lexer's `as_plain_scalar`/`as_single_quoted` judge decoded
//! content and accept styles that lose data when emitted (an interior
//! tab as a plain scalar, edge whitespace inside `'...'`, both of which
//! re-read as a different string).

use std::cell::RefCell;
use std::rc::Rc;

use mlua::{Lua, LuaSerdeExt, MetaMethod, MultiValue, UserData, UserDataMethods, Value};

use yamlito::SyntaxTree;
use yamlito::Yaml;
use yamlito::ast::{AstNode, Node, Scalar, ScalarStyle, Value as YamlValue};
use crate::scripting::codec;
use yamlito::edit::{
    GreenElement, TreeEdit, apply_edit, insert_map_entry, insert_seq_item, mk_node, mk_token,
    push_seq_item, remove_map_entry, remove_seq_item,
};
// The restyling methods take their policy from `quoting`, not `lexer`:
// the lexer's `as_plain_scalar`/`as_single_quoted` judge decoded content
// and permit styles that lose data when *emitted* (an interior tab as a
// plain scalar, edge whitespace inside `'...'`).
use yamlito::quoting::{as_double_quoted, as_plain_scalar, as_single_quoted, quote_scalar_block};
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

/// One step of a cursor's path.
///
/// `Index` is deliberately polymorphic: an integer means "document" on a
/// stream and "element" on a sequence, which is what the Luau surface
/// already meant by `stream[i]` and `seq[i]`. Resolving it against the
/// *parent* node settles which, so traversal never has to look at the
/// tree to decide what a step means.
#[derive(Clone, Debug)]
pub enum Segment {
    /// Sequence element, or document of a stream (0-based internally).
    Index(usize),
    /// Value of the mapping entry with this key.
    Key(String),
    /// The *key* node of the mapping entry with this key. A second edge
    /// out of a mapping, which is what lets `entry:key_node()` be a
    /// path-carrying cursor rather than a cached node.
    KeyNode(String),
}

impl std::fmt::Display for Segment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // 1-based on the way out: the path is a diagnostic, so it is
            // spelled the way the script spelled it.
            Segment::Index(i) => write!(f, "[{}]", i + 1),
            Segment::Key(k) => write!(f, ".{k}"),
            Segment::KeyNode(k) => write!(f, ".{k}<key>"),
        }
    }
}

/// Render a traversed prefix for an error message, e.g. `db.replicas[2]`.
///
/// The leading document step is dropped for a single-document stream —
/// the overwhelmingly common case, where `doc[1].db` would only add
/// noise. A later document keeps its `doc[n]` prefix, since there the
/// index is the useful part.
fn render_path(path: &[Segment]) -> String {
    let mut out = String::new();
    let mut rest = path;
    if let Some((Segment::Index(i), tail)) = path.split_first() {
        if *i > 0 {
            out.push_str(&format!("doc[{}]", i + 1));
        }
        rest = tail;
    }
    for seg in rest {
        out.push_str(&seg.to_string());
    }
    match out.strip_prefix('.') {
        Some(s) if !s.is_empty() => s.to_string(),
        _ if out.is_empty() => "the document root".to_string(),
        _ => out,
    }
}

/// A cursor over a live YAML syntax tree: the shared tree plus the path
/// to a node within it. Cloning yields an independent cursor over the
/// same tree, so traversal is non-destructive and aliasable.
#[derive(Clone)]
pub struct YamlCursor {
    tree: TreeHandle,
    path: Vec<Segment>,
}

fn wrong_kind(method: &str, expected: &str, got: &str) -> mlua::Error {
    err(format!("{method}: not a {expected} (got {got})"))
}

impl YamlCursor {
    fn root(tree: TreeHandle) -> Self {
        Self {
            tree,
            path: Vec::new(),
        }
    }

    /// Extend the path by one step. Never resolves, so never fails.
    fn child(&self, seg: Segment) -> Self {
        let mut next = self.clone();
        next.path.push(seg);
        next
    }

    /// Resolve this cursor's path against the tree's *current* root.
    ///
    /// The tree borrow is taken and released up front — the mutating
    /// paths need `borrow_mut()` afterwards, and holding both would
    /// panic.
    fn resolve(&self) -> Result<SyntaxNode> {
        let mut cur = self.tree.borrow().root().clone();
        for (i, seg) in self.path.iter().enumerate() {
            cur = match step(&cur, seg) {
                Some(n) => n,
                // An integer never names a mapping entry, even one whose key
                // looks numeric: say so rather than "out of range".
                None if matches!(seg, Segment::Index(_)) && kind_of(&cur) == "mapping" => {
                    let Segment::Index(n) = seg else { unreachable!() };
                    return Err(err(format!(
                        "{} is a mapping: address an entry by key (m[\"{}\"]), an integer is a position (at {})",
                        render_path(&self.path[..i]),
                        n + 1,
                        render_path(&self.path[..i])
                    )));
                }
                None => return Err(self.missing(&self.path[..i], seg)),
            };
        }
        Ok(cur)
    }

    /// Split the path into `(parent cursor, last segment)`, for the
    /// self-addressing forms (`remove()`, `update(fn)`) that need to know
    /// how *this* cursor is addressed within its parent collection.
    fn parent_and_last(&self, method: &str) -> Result<(YamlCursor, Segment)> {
        let Some((last, rest)) = self.path.split_last() else {
            return Err(err(format!("{method}: cursor is the document root")));
        };
        Ok((
            YamlCursor {
                tree: self.tree.clone(),
                path: rest.to_vec(),
            },
            last.clone(),
        ))
    }

    /// Resolve, and require the result be of kind `expected`.
    fn resolve_kind(&self, method: &str, expected: &str) -> Result<SyntaxNode> {
        let node = self.resolve()?;
        let got = kind_of(&node);
        if got == expected {
            Ok(node)
        } else {
            Err(wrong_kind(method, expected, got))
        }
    }

    /// Resolve to a collection of either kind, for the methods that take
    /// both (`add`, `add_at`, `remove_at`).
    fn resolve_collection(&self, method: &str) -> Result<(SyntaxNode, &'static str)> {
        let node = self.resolve()?;
        match kind_of(&node) {
            k @ ("mapping" | "sequence") => Ok((node, k)),
            other => Err(wrong_kind(method, "mapping or sequence", other)),
        }
    }

    /// The error for a step that didn't resolve, naming what was missing
    /// and how far the walk got.
    fn missing(&self, traversed: &[Segment], seg: &Segment) -> mlua::Error {
        let at = render_path(traversed);
        match seg {
            Segment::Key(k) | Segment::KeyNode(k) => err(format!("key not found: {k} (at {at})")),
            Segment::Index(i) => err(format!("index out of range: {} (at {at})", i + 1)),
        }
    }
}

/// Resolve one path step against a live node, via the typed AST layer.
fn step(node: &SyntaxNode, seg: &Segment) -> Option<SyntaxNode> {
    match seg {
        Segment::Index(i) => {
            // A stream indexes documents; a sequence indexes elements.
            if node.kind() == SyntaxKind::STREAM {
                rooted_docs(node).into_iter().nth(*i)
            } else {
                resolve_seq_index(node, *i)
            }
        }
        Segment::Key(k) => resolve_map_value(node, k),
        Segment::KeyNode(k) => mapping_key_node(node, k),
    }
}

/// Name the kind of a resolved node, as the Luau surface spells it.
fn kind_of(node: &SyntaxNode) -> &'static str {
    match node.kind() {
        SyntaxKind::BLOCK_MAPPING | SyntaxKind::FLOW_MAPPING => "mapping",
        SyntaxKind::BLOCK_SEQUENCE | SyntaxKind::FLOW_SEQUENCE => "sequence",
        // NULL_SCALAR is an implicit null (`a:`) — a scalar that reads as
        // `ito.null`, not a kind of its own.
        SyntaxKind::SCALAR | SyntaxKind::NULL_SCALAR => "scalar",
        // The *node* kind is ALIAS_NODE; SyntaxKind::ALIAS is the token.
        SyntaxKind::ALIAS_NODE => "alias",
        SyntaxKind::STREAM => "stream",
        _ => "unknown",
    }
}

/// Yielded by mapping iteration (`mapping:entries()`). Exposed to
/// scripts as `YamlEntry` with `:key()`, `:value()`, and `:key_node()`.
///
/// Holds the *owning mapping's* cursor plus the decoded key — the key is
/// the entry's address, not a snapshot of it, so both derived cursors are
/// key-addressed and survive edits to sibling entries. If the entry is
/// later removed, `:key()` still answers (harmlessly) while
/// `:value():read()` raises, which is the honest outcome.
#[derive(Clone)]
pub struct MapEntry {
    mapping: YamlCursor,
    key: String,
}

impl MapEntry {
    fn value(&self) -> YamlCursor {
        self.mapping.child(Segment::Key(self.key.clone()))
    }

    /// Cursor (scalar) at the entry's key node.
    fn key_cursor(&self) -> YamlCursor {
        self.mapping.child(Segment::KeyNode(self.key.clone()))
    }
}

impl UserData for MapEntry {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("key", |_, e, ()| Ok(e.key.clone()));
        methods.add_method("value", |_, e, ()| Ok(e.value()));
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

/// Convert a decoded scalar `Value` into a Lua value.
fn value_to_lua(lua: &Lua, v: &YamlValue) -> Result<Value> {
    Ok(match v {
        YamlValue::Null => lua.null(),
        YamlValue::Bool(b) => Value::Boolean(*b),
        YamlValue::Int(i) => Value::Integer(*i),
        YamlValue::Float(f) => Value::Number(*f),
        YamlValue::String(s) => Value::String(lua.create_string(s)?),
    })
}

/// Scalar text for a scalar `Yaml` value, or `None` for a collection. A
/// string keeps the quote style of the token it replaces (`current`) when that
/// style can represent it; everything else uses the builder's own formatting
/// (so floats match `yaml.to_string`).
fn scalar_text(data: &Yaml, current: Option<SyntaxKind>) -> Option<String> {
    match data {
        Yaml::Seq(_) | Yaml::Map(_) => None,
        Yaml::Str(s) => {
            let style = match current {
                Some(SyntaxKind::SINGLE_QUOTED_SCALAR) => QuoteStyle::Single,
                Some(SyntaxKind::DOUBLE_QUOTED_SCALAR) => QuoteStyle::Double,
                _ => QuoteStyle::Auto,
            };
            let (_, text) = style.encode(s).or_else(|| QuoteStyle::Auto.encode(s))?;
            Some(text)
        }
        other => Some(other.to_yaml_string().trim_end().to_string()),
    }
}

/// Convert a Lua value into a `yamlito::Yaml` value model, ready to
/// splice into a collection (the same conversion `yaml.to_string` uses).
fn lua_to_yaml(lua: &Lua, value: &Value) -> Result<Yaml> {
    codec::to_data(lua, value, "yaml write error")
}

/// Classify already-quoted scalar text by the style it was emitted in,
/// so the token carries the kind matching its own delimiters. The
/// quoting policy chooses the style; this only reads the choice back,
/// which is why every emitter routes through it instead of testing for
/// a leading `'` itself (a test that silently mislabels a double-quoted
/// value as `PLAIN_SCALAR`).
fn classify_scalar_text(text: &str) -> SyntaxKind {
    match text.as_bytes().first() {
        Some(b'\'') => SyntaxKind::SINGLE_QUOTED_SCALAR,
        Some(b'"') => SyntaxKind::DOUBLE_QUOTED_SCALAR,
        _ => SyntaxKind::PLAIN_SCALAR,
    }
}

/// Quote `s` for emission as a scalar, deferring to the crate-wide
/// policy (`plain` → `'single'` → `"double"`) that the builder and the
/// collection-edit paths already share.
///
/// This must not be reimplemented locally: an ad-hoc "needs quotes?"
/// predicate that stops at single-quoted style cannot represent a value
/// containing a newline or edge whitespace, and silently writes one that
/// re-reads as something else (`" pad "` → `pad`).
fn yaml_quote_if_needed(s: &str) -> String {
    quote_scalar_block(s)
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

/// Value cursors for every entry, **key-addressed** so each survives an
/// edit to a sibling entry.
fn mapping_values(cursor: &YamlCursor, node: &SyntaxNode) -> Vec<YamlCursor> {
    mapping_keys(node)
        .into_iter()
        .map(|k| cursor.child(Segment::Key(k)))
        .collect()
}

fn mapping_entries(cursor: &YamlCursor, node: &SyntaxNode) -> Vec<MapEntry> {
    mapping_keys(node)
        .into_iter()
        .map(|key| MapEntry {
            mapping: cursor.clone(),
            key,
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
    if old_key != new_key && mapping_key_index(node, new_key).is_some() {
        return Err(err(format!("duplicate key: {new_key}")));
    }

    let scalar = yamlito::ast::Scalar::cast(key_syntax.clone())
        .ok_or_else(|| err("key is not a scalar node"))?;
    let tok = scalar
        .value_token()
        .ok_or_else(|| err("key scalar has no value token"))?;

    let token_text = yaml_quote_if_needed(new_key);
    let token_kind = classify_scalar_text(&token_text);

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

/// Cursors for the elements of `node` whose kind passes `predicate`.
///
/// Unavoidably **index-addressed**: a sequence element has no name to
/// key on, so each cursor carries the position the element holds now and
/// shifts if an earlier sibling is removed.
fn seq_elements_of_kind(
    cursor: &YamlCursor,
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
        .enumerate()
        .filter(|(_, n)| predicate(n.kind()))
        .map(|(i, _)| cursor.child(Segment::Index(i)))
        .collect()
}

fn scalar_read(lua: &Lua, node: &SyntaxNode) -> Result<Value> {
    let scalar = Scalar::cast(node.clone()).ok_or_else(|| err("not a scalar"))?;
    let v = scalar
        .to_value()
        .map_err(|e| err(format!("decode error: {e:?}")))?;
    value_to_lua(lua, &v)
}

/// Put `token_text` (of kind `token_kind`) in place of the scalar at
/// `node`.
///
/// Swapping the *token* when the kind is unchanged — rather than always
/// replacing the node — is what preserves the scalar's trivia and any
/// preceding node properties (anchor/tag).
fn scalar_splice(
    tree: &TreeHandle,
    node: &SyntaxNode,
    current_tok: yamlito::SyntaxToken,
    token_kind: SyntaxKind,
    token_text: &str,
) {
    let mut tree = tree.borrow_mut();
    if current_tok.kind() == token_kind {
        apply_edit(
            &mut tree,
            TreeEdit::ReplaceToken {
                target: current_tok,
                new: mk_token(token_kind, token_text),
            },
        );
    } else {
        apply_edit(
            &mut tree,
            TreeEdit::ReplaceNode {
                target: node.clone(),
                new: mk_node(
                    SyntaxKind::SCALAR,
                    [GreenElement::Token(mk_token(token_kind, token_text))],
                ),
            },
        );
    }
}

/// Write `data` over the node a cursor resolves to. A scalar over a scalar is
/// a token swap (keeps trivia, anchor/tag and quote style); anything else —
/// a collection on either side, or an implicit null (`key:`) with no token —
/// is re-inserted through the parent collection.
fn write_data(cursor: &YamlCursor, node: &SyntaxNode, data: Yaml) -> Result<()> {
    if let Some(scalar) = Scalar::cast(node.clone())
        && let Some(tok) = scalar.value_token()
        && let Some(text) = scalar_text(&data, Some(tok.kind()))
    {
        let kind = classify_scalar_text(&text);
        scalar_splice(&cursor.tree, node, tok, kind, &text);
        return Ok(());
    }
    let (parent, seg) = cursor.parent_and_last("write")?;
    let (pnode, kind) = parent.resolve_collection("write")?;
    let mut tree = cursor.tree.borrow_mut();
    match (seg, kind) {
        (Segment::Key(k), "mapping") => {
            let i = mapping_key_index(&pnode, &k).ok_or_else(|| err("key not found"))?;
            // insert before, then drop the old entry: removing first could
            // collapse a one-entry block mapping into `{}`.
            let live = insert_map_entry(&mut tree, &pnode, i, &k, data);
            remove_map_entry(&mut tree, &live, i + 1);
        }
        (Segment::Index(i), "sequence") => {
            let live = insert_seq_item(&mut tree, &pnode, i, data);
            remove_seq_item(&mut tree, &live, i + 1);
        }
        _ => return Err(err("write: cannot replace the document root")),
    }
    Ok(())
}

/// The quoting styles a script may ask for. `Auto` is not a style but a
/// choice *between* them: the narrowest one that round-trips, which is
/// the policy `yamlito::quoting` (and so `:write`) already applies.
#[derive(Clone, Copy, PartialEq, Eq)]
enum QuoteStyle {
    Plain,
    Single,
    Double,
    Auto,
}

impl QuoteStyle {
    /// Render the decoded value `s` in this style as raw token text,
    /// paired with the token kind to emit. `None` when the style cannot
    /// represent the value at all — the only honest failure, since
    /// `Double`/`Auto` always can.
    fn encode(self, s: &str) -> Option<(SyntaxKind, String)> {
        match self {
            QuoteStyle::Plain => Some((SyntaxKind::PLAIN_SCALAR, as_plain_scalar(s)?.to_string())),
            QuoteStyle::Single => Some((SyntaxKind::SINGLE_QUOTED_SCALAR, as_single_quoted(s)?)),
            QuoteStyle::Double => Some((SyntaxKind::DOUBLE_QUOTED_SCALAR, as_double_quoted(s))),
            // `quote_scalar_block` picks plain/single/double by the same
            // rules, so auto's result is classified by what it produced
            // rather than re-deciding here.
            QuoteStyle::Auto => {
                let text = quote_scalar_block(s);
                Some((classify_scalar_text(&text), text))
            }
        }
    }
}

/// Name a scalar's current style, so a script can ask before it
/// converts (and can see the block styles the `make_*` methods decline
/// to touch). `"none"` covers the receivers that reach a Scalar cursor
/// without carrying a scalar token — an alias (`*anchor`), which has no
/// style of its own to report.
fn scalar_quote_style(node: &SyntaxNode) -> &'static str {
    match Scalar::cast(node.clone()).and_then(|sc| sc.style()) {
        Some(ScalarStyle::Plain) => "plain",
        Some(ScalarStyle::SingleQuoted) => "single",
        Some(ScalarStyle::DoubleQuoted) => "double",
        Some(ScalarStyle::Literal) => "literal",
        Some(ScalarStyle::Folded) => "folded",
        None => "none",
    }
}

/// Restyle a scalar in place: decode it to its logical string, then
/// re-encode in `style`. Going through the decoded value is what makes
/// every pair of styles reachable — `'it''s'` becomes `"it's"` and back
/// — instead of only the pairs whose raw text happens to transfer.
///
/// Returns `false` (leaving the node untouched) when the conversion
/// isn't available:
///
/// - the node is not a scalar, or has no value token (implicit null);
/// - it is a block scalar (`|`/`>`): its decoded value is multi-line
///   text whose indentation is part of the layout, so rewriting it as a
///   single quoted token would reflow the document rather than restyle a
///   value;
/// - its raw text doesn't decode (a malformed escape);
/// - `style` cannot represent the value (`Plain` for anything but a
///   plain-safe string, `Single` for control characters or edge
///   whitespace).
///
/// Already being in the requested style is *not* a failure: the call is
/// a no-op that reports `true`, since the scalar is in the style the
/// caller asked for.
fn scalar_try_restyle(tree: &TreeHandle, node: &SyntaxNode, style: QuoteStyle) -> bool {
    let Some(scalar) = Scalar::cast(node.clone()) else {
        return false;
    };
    let Some(tok) = scalar.value_token() else {
        return false;
    };
    if matches!(
        tok.kind(),
        SyntaxKind::LITERAL_SCALAR | SyntaxKind::FOLDED_SCALAR
    ) {
        return false;
    }
    let Ok(decoded) = scalar.decoded() else {
        return false;
    };
    let Some((token_kind, token_text)) = style.encode(&decoded) else {
        return false;
    };

    if tok.kind() == token_kind && tok.text() == token_text {
        return true;
    }

    scalar_splice(tree, node, tok, token_kind, &token_text);
    true
}

/// Cursors for each document root in the stream, addressed by document
/// position. A document with no root node is skipped, so the cursor
/// carries the document's own index rather than its position in the
/// result.
fn stream_docs(cursor: &YamlCursor, node: &SyntaxNode) -> Vec<YamlCursor> {
    (0..rooted_docs(node).len())
        .map(|i| cursor.child(Segment::Index(i)))
        .collect()
}

/// Root nodes of the stream's documents that have one. A document without a
/// root (comments only) does not count: `len`, `docs`, `doc` and `stream[i]`
/// all number the documents through this list, so they agree.
fn rooted_docs(node: &SyntaxNode) -> Vec<SyntaxNode> {
    match yamlito::ast::Stream::cast(node.clone()) {
        Some(st) => st
            .documents()
            .filter_map(|d| d.root_node())
            .map(|n| node_syntax(&n))
            .collect(),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// UserData: the unified cursor
// ---------------------------------------------------------------------------

impl UserData for YamlCursor {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // --- kind: the resolved node's kind as a string, and one is_*()
        //     predicate per kind -- both read the same `kind_of` helper, so
        //     `c:kind() == "mapping"` and `c:is_mapping()` cannot disagree.
        //     Both resolve, so both raise on a path that isn't there;
        //     `:exists()` / `:try_kind()` are the non-raising forms. ---
        methods.add_method("kind", |_, this, ()| Ok(kind_of(&this.resolve()?)));
        methods.add_method("try_kind", |_, this, ()| {
            Ok(this.resolve().ok().map(|n| kind_of(&n)))
        });
        methods.add_method("exists", |_, this, ()| Ok(this.resolve().is_ok()));
        for (name, want) in [
            ("is_mapping", "mapping"),
            ("is_sequence", "sequence"),
            ("is_scalar", "scalar"),
            ("is_stream", "stream"),
            ("is_alias", "alias"),
        ] {
            methods.add_method(name, move |_, this, ()| {
                Ok(kind_of(&this.resolve()?) == want)
            });
        }

        // --- len: mapping (entry count), sequence (item count), stream
        //     (document count). ---
        methods.add_method("len", |_, this, ()| this.len());
        methods.add_method("try_len", |_, this, ()| Ok(this.len().ok()));

        // --- to_string: raw source text of the node. The stream cursor
        //     renders the whole (possibly mutated) tree; a sub-cursor
        //     renders its own node, resolved fresh. ---
        methods.add_method("to_string", |_, this, ()| this.render());
        methods.add_method("try_to_string", |_, this, ()| Ok(this.render().ok()));

        // --- keys: mapping only. ---
        methods.add_method("keys", |_, this, ()| {
            Ok(mapping_keys(&this.resolve_kind("keys", "mapping")?))
        });

        // --- values: mapping value cursors in source order. ---
        methods.add_method("values", |_, this, ()| {
            let (node, kind) = this.resolve_collection("values")?;
            Ok(if kind == "mapping" {
                mapping_values(this, &node)
            } else {
                seq_elements_of_kind(this, &node, |_| true)
            })
        });

        // --- entries: mapping iteration helper -> array of YamlEntry. ---
        methods.add_method("entries", |_, this, ()| {
            let node = this.resolve_kind("entries", "mapping")?;
            Ok(mapping_entries(this, &node))
        });

        // --- contains: mapping only. ---
        methods.add_method("contains", |_, this, key: String| {
            Ok(mapping_contains(
                &this.resolve_kind("contains", "mapping")?,
                &key,
            ))
        });

        // --- get: explicit alternative to bracket indexing.
        //     mapping[key] / sequence[i] / stream[i]. Like indexing, it
        //     only extends the path -- it does not resolve. ---
        methods.add_method("get", |_, this, key: Value| index_step(this, key));

        // --- mappings / sequences / scalars: sequence element selectors. ---
        for (name, pred) in [("mappings", 0u8), ("sequences", 1u8), ("scalars", 2u8)] {
            methods.add_method(name, move |_, this, ()| {
                let node = this.resolve_kind(name, "sequence")?;
                Ok(seq_elements_of_kind(this, &node, |k| match pred {
                    0 => matches!(k, SyntaxKind::BLOCK_MAPPING | SyntaxKind::FLOW_MAPPING),
                    1 => matches!(k, SyntaxKind::BLOCK_SEQUENCE | SyntaxKind::FLOW_SEQUENCE),
                    _ => matches!(
                        k,
                        SyntaxKind::SCALAR | SyntaxKind::NULL_SCALAR | SyntaxKind::ALIAS_NODE
                    ),
                }))
            });
        }

        // --- doc / docs: stream navigation. `:doc()` is the first
        //     document, i.e. `stream[1]`. ---
        methods.add_method("doc", |_, this, ()| {
            let node = this.resolve_kind("doc", "stream")?;
            // Resolve eagerly here, unlike bracket indexing: `:doc()`
            // names no index of its own, so "the stream has no document"
            // is a better error than a later `index out of range: 1`.
            if stream_docs(this, &node).is_empty() {
                return Err(err("stream has no document"));
            }
            Ok(this.child(Segment::Index(0)))
        });
        methods.add_method("docs", |_, this, ()| {
            let node = this.resolve_kind("docs", "stream")?;
            Ok(stream_docs(this, &node))
        });

        // --- read / write: scalar only. ---
        methods.add_method("read", |lua, this, ()| {
            scalar_read(lua, &this.resolve_kind("read", "scalar")?)
        });
        methods.add_method("try_read", |lua, this, ()| {
            match this.resolve_kind("try_read", "scalar") {
                Ok(node) => Ok(scalar_read(lua, &node).ok()),
                Err(_) => Ok(None),
            }
        });
        methods.add_method("write", |lua, this, value: Value| this.write(lua, &value));
        methods.add_method("try_write", |lua, this, value: Value| {
            Ok(this.write(lua, &value).is_ok())
        });

        // --- scalar style: one method per style, plus the auto choice
        //     between them. Each returns a bool (whether the conversion
        //     happened) rather than raising on an unrepresentable value --
        //     there is no other natural return value for a restyle, and a
        //     `double`/`auto` call always succeeds for a representable
        //     scalar anyway. Still raises on a non-scalar receiver, since
        //     that is a usage error rather than a "can't represent". ---
        methods.add_method("quote_style", |_, this, ()| {
            Ok(scalar_quote_style(
                &this.resolve_kind("quote_style", "scalar")?,
            ))
        });
        for (name, style) in [
            ("unquote", QuoteStyle::Plain),
            ("single_quote", QuoteStyle::Single),
            ("double_quote", QuoteStyle::Double),
            ("auto_quote", QuoteStyle::Auto),
        ] {
            methods.add_method(name, move |_, this, ()| {
                let node = this.resolve_kind(name, "scalar")?;
                Ok(scalar_try_restyle(&this.tree, &node, style))
            });
        }

        // --- comment accessors: leading comment. Every kind answers;
        //     the stream reads its own, the rest walk up to the nearest
        //     comment-bearing entry. ---
        methods.add_method("comment", |_, this, ()| {
            let node = this.resolve()?;
            Ok(if node.kind() == SyntaxKind::STREAM {
                yamlito::ast::Stream::cast(node)
                    .map(|st| st.leading_comment())
                    .unwrap_or_default()
            } else {
                comment::read_leading(&node)
            })
        });
        methods.add_method("set_comment", |_, this, text: String| {
            let node = this.resolve()?;
            if node.kind() == SyntaxKind::STREAM {
                let stream = yamlito::ast::Stream::cast(node).ok_or_else(|| err("not a stream"))?;
                let mut tree = this.tree.borrow_mut();
                stream
                    .set_leading_comment(&mut tree, Some(&text))
                    .map_err(err)
            } else {
                let mut tree = this.tree.borrow_mut();
                comment::write_leading(&mut tree, &node, &text).map_err(err)
            }
        });
        methods.add_method("clear_comment", |_, this, ()| {
            let node = this.resolve()?;
            if node.kind() == SyntaxKind::STREAM {
                let stream = yamlito::ast::Stream::cast(node).ok_or_else(|| err("not a stream"))?;
                let mut tree = this.tree.borrow_mut();
                stream.set_leading_comment(&mut tree, None).map_err(err)
            } else {
                let mut tree = this.tree.borrow_mut();
                comment::clear_leading(&mut tree, &node);
                Ok(())
            }
        });

        // --- trailing comment accessors: everything but the stream,
        //     which has no same-line trailing-comment concept. ---
        methods.add_method("trailing_comment", |_, this, ()| {
            let node = this.resolve()?;
            if node.kind() == SyntaxKind::STREAM {
                return Err(wrong_kind(
                    "trailing_comment",
                    "entry-bearing node",
                    "stream",
                ));
            }
            Ok(comment::read_trailing(&node))
        });
        methods.add_method("set_trailing_comment", |_, this, text: String| {
            let node = this.resolve()?;
            if node.kind() == SyntaxKind::STREAM {
                return Err(wrong_kind(
                    "set_trailing_comment",
                    "entry-bearing node",
                    "stream",
                ));
            }
            let mut tree = this.tree.borrow_mut();
            comment::write_trailing(&mut tree, &node, &text).map_err(err)
        });
        methods.add_method("clear_trailing_comment", |_, this, ()| {
            let node = this.resolve()?;
            if node.kind() == SyntaxKind::STREAM {
                return Err(wrong_kind(
                    "clear_trailing_comment",
                    "entry-bearing node",
                    "stream",
                ));
            }
            let mut tree = this.tree.borrow_mut();
            comment::clear_trailing(&mut tree, &node);
            Ok(())
        });

        // --- rename_key: mapping only. ---
        methods.add_method(
            "rename_key",
            |_, this, (old_key, new_key): (String, String)| {
                let node = this.resolve_kind("rename_key", "mapping")?;
                mapping_rename_key(&this.tree, &node, &old_key, &new_key)
            },
        );

        // --- raw: the node's own source text, as an opaque handle. Same
        //     concept as HCL's `raw()` -- reserves the name for a future
        //     `set_raw`/verbatim-write escape hatch, unlike `to_string()`
        //     which stays a plain display string. ---
        methods.add_method("raw", |_, this, ()| Ok(RawNode(this.render()?)));
        methods.add_method("try_raw", |_, this, ()| Ok(this.render().ok().map(RawNode)));

        // --- add: mapping (key, value) / sequence (value). Errors on a
        //     duplicate key. The path is unchanged: it still names this
        //     collection, which the next resolve finds in its rebuilt
        //     form. ---
        methods.add_method("add", |lua, this, mut args: MultiValue| {
            if this.resolve()?.kind() == SyntaxKind::STREAM {
                return this.add_document(lua, args);
            }
            let (node, kind) = this.resolve_collection("add")?;
            if kind == "mapping" {
                let key: String = pop_arg(lua, &mut args, "add: key")?;
                let value = pop_value(&mut args, "add", "(key, value) for a mapping")?;
                map_add(lua, &this.tree, &node, usize::MAX, &key, &value)?;
            } else {
                let value = pop_value(&mut args, "add", "(value) for a sequence")?;
                let item = lua_to_yaml(lua, &value)?;
                let mut tree = this.tree.borrow_mut();
                push_seq_item(&mut tree, &node, item);
            }
            Ok(())
        });

        // --- add_at: mapping (index, key, value) / sequence (index,
        //     value). `index` is 1-based, matching Lua's own indexing
        //     convention; out-of-range (past the end) clamps to append.
        //     Errors on a duplicate key. ---
        methods.add_method("add_at", |lua, this, mut args: MultiValue| {
            let (node, kind) = this.resolve_collection("add_at")?;
            let index: i64 = pop_arg(lua, &mut args, "add_at: index")?;
            let pos = zero_based(index)?;
            if kind == "mapping" {
                let key: String = pop_arg(lua, &mut args, "add_at: key")?;
                let value = pop_value(&mut args, "add_at", "(index, key, value) for a mapping")?;
                map_add(lua, &this.tree, &node, pos, &key, &value)?;
            } else {
                let value = pop_value(&mut args, "add_at", "(index, value) for a sequence")?;
                let item = lua_to_yaml(lua, &value)?;
                let mut tree = this.tree.borrow_mut();
                insert_seq_item(&mut tree, &node, pos, item);
            }
            Ok(())
        });

        // --- update: self (update_fn), mapping (key, update_fn), sequence/
        //     mapping-by-position (index, update_fn) -- both `index` forms
        //     accepted, matching `remove`'s `remove(index)`/`remove_at`
        //     duplication, since the index *is* the identifier for a
        //     sequence element. Scalar targets only; a table/collection
        //     target raises. `nil` returned by `update_fn` removes the
        //     entry/element instead of writing it. ---
        methods.add_method("update", |lua, this, mut args: MultiValue| {
            let first = args
                .pop_front()
                .ok_or_else(|| err("update: expected 1 or 2 arguments, got 0"))?;
            match args.pop_front() {
                None => {
                    let update_fn: mlua::Function = mlua::FromLua::from_lua(first, lua)?;
                    this.update_self(lua, update_fn)
                }
                Some(second) => {
                    let update_fn: mlua::Function = mlua::FromLua::from_lua(second, lua)?;
                    match first {
                        Value::Integer(i) => this.update_at(lua, i, update_fn),
                        Value::String(s) => this.update_key(lua, &s.to_string_lossy(), update_fn),
                        other => Err(err(format!(
                            "update: first argument must be an index or key, got {}",
                            other.type_name()
                        ))),
                    }
                }
            }
        });

        // --- update_at: sequence or mapping element by 1-based position. ---
        methods.add_method(
            "update_at",
            |lua, this, (index, update_fn): (i64, mlua::Function)| {
                this.update_at(lua, index, update_fn)
            },
        );

        // --- update_all: mapping (every value) / sequence (every
        //     element), in place. Scalar entries only -- a non-scalar
        //     entry raises, same as `update`. ---
        methods.add_method("update_all", |lua, this, update_fn: mlua::Function| {
            this.update_all(lua, update_fn)
        });

        // --- upsert: like `update(key, fn)`, but a missing key calls
        //     `upsert_fn(nil)` and adds the result instead of raising. ---
        methods.add_method(
            "upsert",
            |lua, this, (key, upsert_fn): (String, mlua::Function)| {
                this.upsert(lua, &key, usize::MAX, upsert_fn)
            },
        );

        // --- upsert_at: as `upsert`, but a newly-added entry lands at the
        //     given position rather than the end. ---
        methods.add_method(
            "upsert_at",
            |lua, this, (index, key, upsert_fn): (i64, String, mlua::Function)| {
                let pos = zero_based(index)?;
                this.upsert(lua, &key, pos, upsert_fn)
            },
        );

        // --- remove: self (), mapping (by key) -> bool. ---
        methods.add_method("remove", |_, this, key: Option<String>| this.remove(key));
        methods.add_method("try_remove", |_, this, key: Option<String>| {
            Ok(this.remove(key).unwrap_or(false))
        });

        // --- remove_at: mapping / sequence (by 1-based index, matching
        //     Lua's own indexing convention; out-of-range is a no-op). ---
        methods.add_method("remove_at", |_, this, index: i64| {
            let pos = zero_based(index)?;
            let (node, kind) = this.resolve_collection("remove_at")?;
            let len = if kind == "mapping" { mapping_len(&node) } else { seq_len(&node) };
            if pos as i64 >= len {
                return Err(err(format!("index out of range: {index}")));
            }
            let mut tree = this.tree.borrow_mut();
            if kind == "mapping" {
                remove_map_entry(&mut tree, &node, pos);
            } else {
                remove_seq_item(&mut tree, &node, pos);
            }
            Ok(())
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

impl YamlCursor {
    /// `write(value)`: any value over any node but the stream. A key node
    /// takes a string and renames the key.
    fn write(&self, lua: &Lua, value: &Value) -> Result<()> {
        if let Some(Segment::KeyNode(old)) = self.path.last() {
            let Value::String(new) = value else {
                return Err(err("write: a key must be a string"));
            };
            let (parent, _) = self.parent_and_last("write")?;
            let node = parent.resolve_kind("write", "mapping")?;
            return mapping_rename_key(&self.tree, &node, old, &new.to_str()?).map(|_| ());
        }
        let node = self.resolve()?;
        if node.kind() == SyntaxKind::STREAM {
            return Err(wrong_kind("write", "node below a document", "stream"));
        }
        write_data(self, &node, lua_to_yaml(lua, value)?)
    }

    /// `stream:add(value)`: append `value` as a new document (the first one
    /// in an empty stream). Re-parses, so comments survive but the tree is
    /// rebuilt; existing cursors stay valid (they are paths).
    fn add_document(&self, lua: &Lua, mut args: MultiValue) -> Result<()> {
        let value = pop_value(&mut args, "add", "(value) for a stream")?;
        let doc = lua_to_yaml(lua, &value)?.to_yaml_string();
        let mut text = self.tree.borrow().emit();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        if !rooted_docs(&self.resolve()?).is_empty() {
            text.push_str("---\n");
        }
        text.push_str(&doc);
        let tree = yamlito::parse(&text).map_err(|e| err(format!("yaml parse error: {e}")))?;
        *self.tree.borrow_mut() = tree;
        Ok(())
    }

    /// Entry/item/document count of the resolved collection.
    fn len(&self) -> Result<i64> {
        let node = self.resolve()?;
        Ok(match kind_of(&node) {
            "mapping" => mapping_len(&node),
            "sequence" => seq_len(&node),
            "stream" => rooted_docs(&node).len() as i64,
            other => return Err(wrong_kind("len", "collection", other)),
        })
    }

    /// Source text: the whole tree for the stream cursor, this node's own
    /// text otherwise. Both read the tree as it is *now*.
    fn render(&self) -> Result<String> {
        let node = self.resolve()?;
        Ok(if node.kind() == SyntaxKind::STREAM {
            self.tree.borrow().emit()
        } else {
            node.text().to_string()
        })
    }

    /// Read-modify-write this scalar cursor via `update_fn`: decode,
    /// call, write back the result (or remove the entry if `update_fn`
    /// returns `nil`). Requires the cursor to resolve to a scalar; a
    /// mapping/sequence/stream target raises.
    /// Returns whether the entry was removed.
    fn update_scalar(&self, lua: &Lua, update_fn: mlua::Function) -> Result<bool> {
        let node = self.resolve_kind("update", "scalar")?;
        let current = scalar_read(lua, &node)?;
        let next = update_fn.call::<Value>(current)?;
        if matches!(next, Value::Nil) {
            self.remove(None)
        } else {
            self.write(lua, &next).map(|_| false)
        }
    }

    /// `update(update_fn)`: self-update, addressed by this cursor's own
    /// path segment within its parent.
    fn update_self(&self, lua: &Lua, update_fn: mlua::Function) -> Result<()> {
        self.update_scalar(lua, update_fn).map(|_| ())
    }

    /// `update(key, update_fn)` (mapping).
    fn update_key(&self, lua: &Lua, key: &str, update_fn: mlua::Function) -> Result<()> {
        self.resolve_kind("update", "mapping")?;
        self.child(Segment::Key(key.to_string()))
            .update_scalar(lua, update_fn)
            .map(|_| ())
    }

    /// `update_at(index, update_fn)`: sequence element, or mapping entry,
    /// by 1-based position.
    fn update_at(&self, lua: &Lua, index: i64, update_fn: mlua::Function) -> Result<()> {
        let (node, kind) = self.resolve_collection("update_at")?;
        if kind == "sequence" {
            let pos = zero_based(index)?;
            self.child(Segment::Index(pos))
                .update_scalar(lua, update_fn)
                .map(|_| ())
        } else {
            let pos = zero_based(index)?;
            let key = mapping_keys(&node)
                .into_iter()
                .nth(pos)
                .ok_or_else(|| err(format!("index out of range: {index}")))?;
            self.child(Segment::Key(key))
                .update_scalar(lua, update_fn)
                .map(|_| ())
        }
    }

    /// `update_all(update_fn)`: applies `update_fn` to every mapping value
    /// or sequence element, in place. Scalar entries only, like `update`.
    fn update_all(&self, lua: &Lua, update_fn: mlua::Function) -> Result<()> {
        let (node, kind) = self.resolve_collection("update_all")?;
        if kind == "mapping" {
            for key in mapping_keys(&node) {
                self.child(Segment::Key(key))
                    .update_scalar(lua, update_fn.clone())?;
            }
        } else {
            // Front-to-back (callbacks see document order); a removed element
            // shifts its successors down, so the position only advances when
            // the element stayed.
            let (mut pos, mut len) = (0usize, seq_len(&node) as usize);
            while pos < len {
                if !self
                    .child(Segment::Index(pos))
                    .update_scalar(lua, update_fn.clone())?
                {
                    pos += 1;
                } else {
                    len -= 1;
                }
            }
        }
        Ok(())
    }

    /// `upsert(key, upsert_fn)` / `upsert_at(index, key, upsert_fn)`:
    /// like `update`, but a missing key calls `upsert_fn(nil)` and adds
    /// the result (at `at`, or appended if `at == usize::MAX`) instead of
    /// raising.
    fn upsert(&self, lua: &Lua, key: &str, at: usize, upsert_fn: mlua::Function) -> Result<()> {
        let node = self.resolve_kind("upsert", "mapping")?;
        if mapping_key_index(&node, key).is_some() {
            self.child(Segment::Key(key.to_string()))
                .update_scalar(lua, upsert_fn)
                .map(|_| ())
        } else {
            let next = upsert_fn.call::<Value>(Value::Nil)?;
            if matches!(next, Value::Nil) {
                return Ok(());
            }
            map_add(lua, &self.tree, &node, at, key, &next)?;
            Ok(())
        }
    }

    /// `remove()` (self, addressed by this cursor's own path within its
    /// parent) / `remove(key)` (mapping, by key) -> whether it was found.
    fn remove(&self, key: Option<String>) -> Result<bool> {
        match key {
            Some(key) => {
                let node = self.resolve_kind("remove", "mapping")?;
                let Some(i) = mapping_key_index(&node, &key) else {
                    return Ok(false);
                };
                let mut tree = self.tree.borrow_mut();
                remove_map_entry(&mut tree, &node, i);
                Ok(true)
            }
            None => {
                let (parent, seg) = self.parent_and_last("remove")?;
                match seg {
                    Segment::Key(k) | Segment::KeyNode(k) => parent.remove(Some(k)),
                    Segment::Index(i) => {
                        let (node, kind) = parent.resolve_collection("remove")?;
                        let len = if kind == "mapping" { mapping_len(&node) } else { seq_len(&node) };
                        if i as i64 >= len {
                            return Ok(false);
                        }
                        let mut tree = self.tree.borrow_mut();
                        if kind == "mapping" {
                            remove_map_entry(&mut tree, &node, i);
                        } else {
                            remove_seq_item(&mut tree, &node, i);
                        }
                        Ok(true)
                    }
                }
            }
        }
    }
}

/// An opaque handle over a node's exact source text, as returned by
/// `:raw()`. Distinct from a plain Lua string returned by `:to_string()`
/// so a future `:write(raw)`/`:set_raw(raw)` escape hatch can recognise
/// "write this back verbatim" without ambiguity against an ordinary
/// string value.
#[derive(Clone)]
pub struct RawNode(String);

impl std::fmt::Display for RawNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl UserData for RawNode {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("to_string", |_, this, ()| Ok(this.0.clone()));
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.0.clone()));
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

/// The last positional argument: required, and nothing may follow it.
fn pop_value(args: &mut MultiValue, method: &str, expected: &str) -> Result<Value> {
    let v = args
        .pop_front()
        .ok_or_else(|| err(format!("{method}: expected {expected}")))?;
    if !args.is_empty() {
        return Err(err(format!("{method}: too many arguments, expected {expected}")));
    }
    Ok(v)
}

/// Bracket-index / `:get()` step: `mapping[string]`, `sequence[int]`,
/// `stream[int]`.
///
/// This only extends the path -- it never touches the tree, so a missing
/// key or an out-of-range index is not an error *here*; it surfaces at
/// the next read or write, naming the path it got through. That is what
/// lets a cursor outlive edits made through its siblings.
///
/// A string names a mapping key and an integer names a position; which
/// container the position indexes (a sequence's elements or a stream's
/// documents) is settled when the path resolves, against the parent's
/// own kind.
fn index_step(this: &YamlCursor, key: Value) -> Result<YamlCursor> {
    match key {
        Value::String(s) => Ok(this.child(Segment::Key(s.to_string_lossy()))),
        Value::Integer(i) => Ok(this.child(Segment::Index(zero_based(i)?))),
        Value::Number(f) if f.fract() == 0.0 => {
            Ok(this.child(Segment::Index(zero_based(f as i64)?)))
        }
        other => Err(err(format!(
            "index must be a string key or an integer, got {}",
            other.type_name()
        ))),
    }
}

/// Parse `text` into a shared mutable tree and return a cursor on its
/// root (the stream), with an empty path. Parse failures surface as Lua
/// runtime errors.
pub fn edit(text: &str, name: Option<&str>) -> Result<YamlCursor> {
    let tree = yamlito::parse(text).map_err(|e| match name {
        Some(n) => err(format!("yaml parse error in {n}: {e}")),
        None => err(format!("yaml parse error: {e}")),
    })?;
    Ok(YamlCursor::root(Rc::new(RefCell::new(tree))))
}

/// Register the `yaml.edit` entry point on the `yaml` module table.
/// The unified [`YamlCursor`] / [`MapEntry`] userdata carry their own
/// metatables, so no separate type registration is needed.
pub fn register(lua: &Lua, module: &mlua::Table) -> Result<()> {
    module.set(
        "edit",
        lua.create_function(|_, (text, name): (String, Option<String>)| edit(&text, name.as_deref()))?,
    )?;
    Ok(())
}
