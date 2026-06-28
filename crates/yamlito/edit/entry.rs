//! Structural entry insertion/removal for mappings (block and flow).
//!
//! The mapping counterpart to `seq.rs`: the single shared primitives for
//! adding or dropping a mapping entry, owning the trivia/indent fix-up
//! once so callers (builder, fixers, comment placement, the scripting
//! cursor) don't each reinvent it. Both operate at the green-node level
//! and apply via `TreeEdit::ReplaceNode` on the collection.
//!
//! Two layouts are handled uniformly behind one entry point:
//!
//! - **Block** (`a: 1\nb: 2`): entries are `BLOCK_MAPPING_ENTRY` nodes
//!   separated by NEWLINE siblings; line-start entries own their leading
//!   indent WHITESPACE. Block layout shares its bookkeeping with the
//!   sequence side via [`crate::edit::collection`].
//! - **Flow** (`{a: 1, b: 2}`): entries are `FLOW_MAPPING_ENTRY` nodes
//!   between `{`/`}`, separated by `,` plus optional WHITESPACE. We
//!   sample the existing separator so `{a:1,b:2}` stays tight and
//!   `{a: 1, b: 2}` stays spaced.
//!
//! A block mapping can never be empty (an empty mapping is only spelled
//! `{}` in flow), so removing the *last* block entry collapses the whole
//! `BLOCK_MAPPING` into a flow `{}`, keeping the tree valid and
//! round-trippable (mirroring the `BLOCK_SEQUENCE` → `[]` collapse).
//!
//! Entry fragments are produced through the value-model builder
//! (strategy A): we serialize the entry to canonical YAML, parse it, and
//! splice the resulting green subtree — so an inserted entry is always
//! parse- and round-trip-consistent.

use rowan::NodeOrToken;

use crate::SyntaxTree;
use crate::builder::Yaml;
use crate::edit::collection::{
    block_insert, block_remove, collapse_block_collection_to_empty, collection_col,
    entry_child_indices, replace_node_live,
};
use crate::edit::{GreenElement, mk_node, mk_token, splice_children_green};
use crate::quoting::quote_scalar_flow;
use crate::syntax::tree_utils::indent_green_continuations;
use crate::syntax::{SyntaxKind, SyntaxNode};

/// Insert `key: value` into the mapping `map` at entry position `index`
/// (clamped to the entry count). Dispatches on the node kind: a
/// `BLOCK_MAPPING` gets an entry on its own line, indented to the
/// mapping's column; a `FLOW_MAPPING` gets a `{...}` entry with
/// separator spacing matching its neighbours.
///
/// `map` must be a `BLOCK_MAPPING` or `FLOW_MAPPING` belonging to
/// `tree`. Returns the *live* mapping node after the edit — rowan's
/// `ReplaceNode` detaches the old node, so the caller should hold this
/// returned handle instead of their old `map`.
pub fn insert_map_entry(
    tree: &mut SyntaxTree,
    map: &SyntaxNode,
    index: usize,
    key: &str,
    value: Yaml,
) -> SyntaxNode {
    match map.kind() {
        SyntaxKind::BLOCK_MAPPING => insert_block_entry(tree, map, index, key, value),
        SyntaxKind::FLOW_MAPPING => insert_flow_entry(tree, map, index, key, value),
        other => {
            debug_assert!(false, "insert_map_entry: not a mapping: {other:?}");
            map.clone()
        }
    }
}

/// Remove the entry at position `index` from `map`, together with the
/// adjacent separator (NEWLINE for block, `,`/WHITESPACE for flow) that
/// joins it to the rest, so no stray separator is left behind. No-op
/// when out of range.
///
/// Removing the last remaining entry of a *block* mapping converts the
/// whole node into an empty flow `{}` (a block mapping cannot be empty);
/// removing the last entry of a flow mapping leaves `{}`.
///
/// Returns the *live* mapping node after the edit — the new node a
/// caller should hold instead of their old `map`. When the last block
/// entry is removed this is the freshly inserted flow `{}` node; for an
/// out-of-range no-op it is `map` unchanged.
pub fn remove_map_entry(tree: &mut SyntaxTree, map: &SyntaxNode, index: usize) -> SyntaxNode {
    match map.kind() {
        SyntaxKind::BLOCK_MAPPING => remove_block_entry(tree, map, index),
        SyntaxKind::FLOW_MAPPING => remove_flow_entry(tree, map, index),
        other => {
            debug_assert!(false, "remove_map_entry: not a mapping: {other:?}");
            map.clone()
        }
    }
}

// ---------------------------------------------------------------------------
// Block mappings
// ---------------------------------------------------------------------------

fn insert_block_entry(
    tree: &mut SyntaxTree,
    map: &SyntaxNode,
    index: usize,
    key: &str,
    value: Yaml,
) -> SyntaxNode {
    let col = collection_col(map, SyntaxKind::BLOCK_MAPPING_ENTRY);
    let entry_green = build_block_entry_green(key, value, col);
    block_insert(
        tree,
        map,
        index,
        SyntaxKind::BLOCK_MAPPING_ENTRY,
        col,
        entry_green,
    )
}

fn remove_block_entry(tree: &mut SyntaxTree, map: &SyntaxNode, index: usize) -> SyntaxNode {
    block_remove(
        tree,
        map,
        index,
        SyntaxKind::BLOCK_MAPPING_ENTRY,
        // A block mapping cannot be empty, so the last entry collapses the
        // whole node to a flow `{}`.
        |tree, map| {
            collapse_block_collection_to_empty(
                tree,
                map,
                SyntaxKind::FLOW_MAPPING,
                (SyntaxKind::L_BRACE, "{"),
                (SyntaxKind::R_BRACE, "}"),
            )
        },
    )
}

/// Build the green `BLOCK_MAPPING_ENTRY` for `key: value`, indented for a
/// mapping at column `col`. We serialize a single-entry mapping, parse
/// its CANONICAL column-0 text (which the builder guarantees parses),
/// lift its entry subtree, then push `col` extra spaces onto every
/// interior continuation line via [`indent_green_continuations`] so a
/// multi-line value aligns under a mapping at `col`.
///
/// The entry's own first line stays bare: the caller supplies its leading
/// indent as a sibling WHITESPACE token (see [`block_insert`]).
fn build_block_entry_green(key: &str, value: Yaml, col: usize) -> rowan::GreenNode {
    let fragment = Yaml::Map(vec![(key.to_string(), value)]);
    let text = fragment.to_yaml_string();
    let tree = crate::parse(&text).expect("builder fragment must parse");
    let entry = tree
        .root()
        .descendants()
        .find(|n| n.kind() == SyntaxKind::BLOCK_MAPPING_ENTRY)
        .expect("fragment has an entry");
    indent_green_continuations(&entry, col)
}

// ---------------------------------------------------------------------------
// Flow mappings
// ---------------------------------------------------------------------------

/// Build the green `FLOW_MAPPING_ENTRY` for `key: value` rendered inline.
/// We serialize the pair as a flow entry, parse `{key: value}`, and lift
/// the entry subtree. Both key and value go through the strict flow
/// quoting path ([`flow_entry_text`]) so flow terminators and the `: `
/// indicator can never leak through and corrupt the surrounding mapping.
fn build_flow_entry_green(key: &str, value: Yaml) -> rowan::GreenNode {
    let text = format!("{{{}}}\n", flow_entry_text(key, &value));
    let tree = crate::parse(&text).expect("flow entry fragment must parse");
    tree.root()
        .descendants()
        .find(|n| n.kind() == SyntaxKind::FLOW_MAPPING_ENTRY)
        .expect("fragment has a flow mapping entry")
        .green()
        .into_owned()
}

/// Render `key: value` as inline flow text suitable for placing inside
/// `{}`. The key and any string value are quoted with the strict flow
/// policy ([`quote_scalar_flow`]); collections recurse through
/// [`flow_value_text`]. Quoting the key AND value protects against the
/// flow terminators (`,` `[` `]` `{` `}`) and the `: ` mapping indicator
/// corrupting the surrounding collection (FINDINGS.md B2/B3).
fn flow_entry_text(key: &str, value: &Yaml) -> String {
    format!("{}: {}", quote_scalar_flow(key), flow_value_text(value))
}

/// Render a `Yaml` value as inline flow text. Strings are flow-quoted;
/// other scalars use the builder's canonical form; collections recurse,
/// quoting keys *and* values the same way.
fn flow_value_text(value: &Yaml) -> String {
    match value {
        Yaml::Str(s) => quote_scalar_flow(s),
        Yaml::Null | Yaml::Bool(_) | Yaml::Int(_) | Yaml::Float(_) => {
            value.to_yaml_string().trim_end_matches('\n').to_string()
        }
        Yaml::Seq(items) => {
            let inner: Vec<String> = items.iter().map(flow_value_text).collect();
            format!("[{}]", inner.join(", "))
        }
        Yaml::Map(pairs) => {
            let inner: Vec<String> = pairs.iter().map(|(k, v)| flow_entry_text(k, v)).collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

/// The separator between flow-mapping entries, sampled from the existing
/// inter-entry whitespace so new entries blend in (`{a: 1, b: 2}` →
/// `", "`; `{a:1,b:2}` → `""`). Single-entry / empty mappings default to
/// `", "`.
fn flow_sep_ws(map: &SyntaxNode) -> String {
    let children: Vec<_> = map.children_with_tokens().collect();
    let comma_pos = children
        .iter()
        .position(|el| matches!(el, NodeOrToken::Token(t) if t.kind() == SyntaxKind::COMMA));
    if let Some(i) = comma_pos {
        if let Some(NodeOrToken::Token(t)) = children.get(i + 1)
            && t.kind() == SyntaxKind::WHITESPACE
        {
            return t.text().to_string();
        }
        // Comma directly followed by the next entry: tight.
        return String::new();
    }
    " ".to_string()
}

/// Tokens that follow a comma before the next entry (the sampled
/// whitespace, if any).
fn flow_sep_after_comma(ws: &str) -> Vec<GreenElement> {
    if ws.is_empty() {
        Vec::new()
    } else {
        vec![NodeOrToken::Token(mk_token(SyntaxKind::WHITESPACE, ws))]
    }
}

fn insert_flow_entry(
    tree: &mut SyntaxTree,
    map: &SyntaxNode,
    index: usize,
    key: &str,
    value: Yaml,
) -> SyntaxNode {
    let entry_green = build_flow_entry_green(key, value);
    let entry_indices = entry_child_indices(map, SyntaxKind::FLOW_MAPPING_ENTRY);
    let ws = flow_sep_ws(map);

    let children: Vec<_> = map.children_with_tokens().collect();
    let r_brace_idx = children
        .iter()
        .position(|el| matches!(el, NodeOrToken::Token(t) if t.kind() == SyntaxKind::R_BRACE));

    let comma = || NodeOrToken::Token(mk_token(SyntaxKind::COMMA, ","));

    if entry_indices.is_empty() {
        // Empty `{}` — drop the entry just before `}` with no separator.
        let at = r_brace_idx.unwrap_or(children.len());
        let frag = vec![NodeOrToken::Node(entry_green)];
        let new_green = splice_children_green(map, at..at, frag);
        return replace_node_live(tree, map, new_green);
    }

    let (at, fragment): (usize, Vec<GreenElement>) = if index >= entry_indices.len() {
        // Append after the last entry: `,<sep>entry`.
        let last = *entry_indices.last().unwrap();
        let at = last + 1;
        let mut frag = vec![comma()];
        frag.extend(flow_sep_after_comma(&ws));
        frag.push(NodeOrToken::Node(entry_green));
        (at, frag)
    } else {
        // Insert before the entry at `index`: `entry,<sep>` so the
        // existing entry keeps its own position.
        let at = entry_indices[index];
        let mut frag = vec![NodeOrToken::Node(entry_green), comma()];
        frag.extend(flow_sep_after_comma(&ws));
        (at, frag)
    };

    let new_green = splice_children_green(map, at..at, fragment);
    replace_node_live(tree, map, new_green)
}

fn remove_flow_entry(tree: &mut SyntaxTree, map: &SyntaxNode, index: usize) -> SyntaxNode {
    let children: Vec<_> = map.children_with_tokens().collect();
    let entry_positions: Vec<usize> = children
        .iter()
        .enumerate()
        .filter(|(_, el)| {
            matches!(el, NodeOrToken::Node(n) if n.kind() == SyntaxKind::FLOW_MAPPING_ENTRY)
        })
        .map(|(i, _)| i)
        .collect();
    let Some(&entry_idx) = entry_positions.get(index) else {
        return map.clone();
    };

    // Removing the only entry: collapse to a clean `{}`, dropping any
    // interior trivia.
    if entry_positions.len() == 1 {
        let empty = mk_node(
            SyntaxKind::FLOW_MAPPING,
            [
                NodeOrToken::Token(mk_token(SyntaxKind::L_BRACE, "{")),
                NodeOrToken::Token(mk_token(SyntaxKind::R_BRACE, "}")),
            ],
        );
        return replace_node_live(tree, map, empty);
    }

    let is_comma = |i: usize| matches!(children.get(i), Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::COMMA);
    let is_ws = |i: usize| matches!(children.get(i), Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::WHITESPACE);

    let mut start = entry_idx;
    let mut end = entry_idx + 1;
    if index + 1 < entry_positions.len() {
        // Not the last entry: swallow the following `,` and any WHITESPACE
        // up to the next entry, so the successor keeps its leading spacing.
        if is_comma(end) {
            end += 1;
        }
        while is_ws(end) {
            end += 1;
        }
    } else {
        // Last (but not only) entry: swallow the *preceding* `,` and the
        // whitespace around it, so no dangling separator remains.
        while start > 0 && is_ws(start - 1) {
            start -= 1;
        }
        if start > 0 && is_comma(start - 1) {
            start -= 1;
        }
        while start > 0 && is_ws(start - 1) {
            start -= 1;
        }
    }

    let new_green = splice_children_green(map, start..end, Vec::new());
    replace_node_live(tree, map, new_green)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AstNode, Scalar};

    fn map_node(tree: &SyntaxTree) -> SyntaxNode {
        tree.root()
            .descendants()
            .find(|n| n.kind() == SyntaxKind::BLOCK_MAPPING)
            .expect("block mapping")
    }

    fn flow_map_node(tree: &SyntaxTree) -> SyntaxNode {
        tree.root()
            .descendants()
            .find(|n| n.kind() == SyntaxKind::FLOW_MAPPING)
            .expect("flow mapping")
    }

    fn reparse(tree: &SyntaxTree) {
        crate::parse(&tree.emit()).expect("emitted text must reparse");
    }

    // ---- block: insert ----

    #[test]
    fn insert_at_end() {
        let mut tree = crate::parse("a: 1\nb: 2\n").unwrap();
        let map = map_node(&tree);
        insert_map_entry(&mut tree, &map, 99, "c", Yaml::Int(3));
        assert_eq!(tree.emit(), "a: 1\nb: 2\nc: 3\n");
        reparse(&tree);
    }

    #[test]
    fn insert_at_front() {
        let mut tree = crate::parse("b: 2\n").unwrap();
        let map = map_node(&tree);
        insert_map_entry(&mut tree, &map, 0, "a", Yaml::Int(1));
        assert_eq!(tree.emit(), "a: 1\nb: 2\n");
    }

    #[test]
    fn insert_string_value_quoted_when_needed() {
        let mut tree = crate::parse("a: 1\n").unwrap();
        let map = map_node(&tree);
        insert_map_entry(&mut tree, &map, 99, "b", Yaml::Str("true".into()));
        assert_eq!(tree.emit(), "a: 1\nb: 'true'\n");
    }

    #[test]
    fn insert_nested_map_value() {
        let mut tree = crate::parse("a: 1\n").unwrap();
        let map = map_node(&tree);
        insert_map_entry(
            &mut tree,
            &map,
            99,
            "b",
            Yaml::Map(vec![("c".into(), Yaml::Int(2))]),
        );
        assert_eq!(tree.emit(), "a: 1\nb:\n  c: 2\n");
        reparse(&tree);
    }

    #[test]
    fn insert_into_nested_mapping_indents() {
        let mut tree = crate::parse("outer:\n  a: 1\n").unwrap();
        // Target the inner mapping.
        let inner = tree
            .root()
            .descendants()
            .filter(|n| n.kind() == SyntaxKind::BLOCK_MAPPING)
            .find(|n| collection_col(n, SyntaxKind::BLOCK_MAPPING_ENTRY) == 2)
            .expect("inner map");
        insert_map_entry(&mut tree, &inner, 99, "b", Yaml::Int(2));
        assert_eq!(tree.emit(), "outer:\n  a: 1\n  b: 2\n");
        reparse(&tree);
    }

    /// A multi-line value (nested map) inserted into a nested mapping is
    /// reindented at the green level, aligning its continuation lines.
    #[test]
    fn insert_multiline_map_value_nested_indents() {
        let mut tree = crate::parse("outer:\n  a: 1\n").unwrap();
        let inner = tree
            .root()
            .descendants()
            .filter(|n| n.kind() == SyntaxKind::BLOCK_MAPPING)
            .find(|n| collection_col(n, SyntaxKind::BLOCK_MAPPING_ENTRY) == 2)
            .expect("inner map");
        insert_map_entry(
            &mut tree,
            &inner,
            99,
            "b",
            Yaml::Map(vec![("c".into(), Yaml::Int(2)), ("d".into(), Yaml::Int(3))]),
        );
        assert_eq!(tree.emit(), "outer:\n  a: 1\n  b:\n    c: 2\n    d: 3\n");
        reparse(&tree);
    }

    /// The returned node is the live block mapping and supports a chained
    /// second edit, without re-finding it in the tree.
    #[test]
    fn insert_returns_live_node_for_chained_edit() {
        let mut tree = crate::parse("a: 1\n").unwrap();
        let map = map_node(&tree);
        let new = insert_map_entry(&mut tree, &map, 99, "b", Yaml::Int(2));
        assert_eq!(new.kind(), SyntaxKind::BLOCK_MAPPING);
        let n2 = insert_map_entry(&mut tree, &new, 99, "c", Yaml::Int(3));
        assert_eq!(n2.kind(), SyntaxKind::BLOCK_MAPPING);
        assert_eq!(tree.emit(), "a: 1\nb: 2\nc: 3\n");
        reparse(&tree);
    }

    // ---- block: remove ----

    #[test]
    fn remove_middle_entry() {
        let mut tree = crate::parse("a: 1\nb: 2\nc: 3\n").unwrap();
        let map = map_node(&tree);
        remove_map_entry(&mut tree, &map, 1);
        assert_eq!(tree.emit(), "a: 1\nc: 3\n");
        reparse(&tree);
    }

    #[test]
    fn remove_last_entry() {
        let mut tree = crate::parse("a: 1\nb: 2\n").unwrap();
        let map = map_node(&tree);
        remove_map_entry(&mut tree, &map, 1);
        assert_eq!(tree.emit(), "a: 1\n");
        reparse(&tree);
    }

    #[test]
    fn remove_out_of_range_is_noop() {
        let mut tree = crate::parse("a: 1\n").unwrap();
        let map = map_node(&tree);
        remove_map_entry(&mut tree, &map, 5);
        assert_eq!(tree.emit(), "a: 1\n");
    }

    // ---- block: emptied-by-removal collapses to flow `{}` ----

    #[test]
    fn remove_only_entry_collapses_to_flow_empty_under_key() {
        let mut tree = crate::parse("key:\n  a: 1\n").unwrap();
        // Target the inner mapping (the value of `key`).
        let inner = tree
            .root()
            .descendants()
            .filter(|n| n.kind() == SyntaxKind::BLOCK_MAPPING)
            .find(|n| collection_col(n, SyntaxKind::BLOCK_MAPPING_ENTRY) == 2)
            .expect("inner map");
        remove_map_entry(&mut tree, &inner, 0);
        assert_eq!(tree.emit(), "key: {}\n");
        reparse(&tree);
    }

    #[test]
    fn remove_only_entry_root_collapses_to_flow_empty() {
        let mut tree = crate::parse("a: 1\n").unwrap();
        let map = map_node(&tree);
        remove_map_entry(&mut tree, &map, 0);
        assert_eq!(tree.emit(), "{}\n");
        reparse(&tree);
    }

    /// The collapse-to-`{}` path returns the live FLOW_MAPPING node, so a
    /// chained insert can grow it without re-finding it.
    #[test]
    fn remove_only_entry_returns_live_flow_node() {
        let mut tree = crate::parse("key:\n  a: 1\n").unwrap();
        let inner = tree
            .root()
            .descendants()
            .filter(|n| n.kind() == SyntaxKind::BLOCK_MAPPING)
            .find(|n| collection_col(n, SyntaxKind::BLOCK_MAPPING_ENTRY) == 2)
            .expect("inner map");
        let empty = remove_map_entry(&mut tree, &inner, 0);
        assert_eq!(tree.emit(), "key: {}\n");
        assert_eq!(empty.kind(), SyntaxKind::FLOW_MAPPING);
        // Grow the returned `{}` node directly.
        let grown = insert_map_entry(&mut tree, &empty, 99, "b", Yaml::Int(2));
        assert_eq!(grown.kind(), SyntaxKind::FLOW_MAPPING);
        assert_eq!(tree.emit(), "key: {b: 2}\n");
        reparse(&tree);
    }

    #[test]
    fn inserted_entry_is_readable() {
        let mut tree = crate::parse("a: 1\n").unwrap();
        let map = map_node(&tree);
        insert_map_entry(&mut tree, &map, 99, "b", Yaml::Int(2));
        // The new value parses as an explicit scalar.
        let scalar = tree
            .root()
            .descendants()
            .filter_map(|n| Scalar::cast(n))
            .find(|s| s.raw_text().as_deref() == Some("2"));
        assert!(scalar.is_some());
    }

    // ---- flow: insert ----

    #[test]
    fn flow_insert_at_end_with_spaces() {
        let mut tree = crate::parse("{a: 1, b: 2}\n").unwrap();
        let map = flow_map_node(&tree);
        insert_map_entry(&mut tree, &map, 99, "c", Yaml::Int(3));
        assert_eq!(tree.emit(), "{a: 1, b: 2, c: 3}\n");
        reparse(&tree);
    }

    #[test]
    fn flow_insert_at_end_tight() {
        let mut tree = crate::parse("{a: 1,b: 2}\n").unwrap();
        let map = flow_map_node(&tree);
        insert_map_entry(&mut tree, &map, 99, "c", Yaml::Int(3));
        assert_eq!(tree.emit(), "{a: 1,b: 2,c: 3}\n");
        reparse(&tree);
    }

    #[test]
    fn flow_insert_at_front() {
        let mut tree = crate::parse("{b: 2, c: 3}\n").unwrap();
        let map = flow_map_node(&tree);
        insert_map_entry(&mut tree, &map, 0, "a", Yaml::Int(1));
        assert_eq!(tree.emit(), "{a: 1, b: 2, c: 3}\n");
        reparse(&tree);
    }

    #[test]
    fn flow_insert_into_empty() {
        let mut tree = crate::parse("{}\n").unwrap();
        let map = flow_map_node(&tree);
        insert_map_entry(&mut tree, &map, 0, "a", Yaml::Int(1));
        assert_eq!(tree.emit(), "{a: 1}\n");
        reparse(&tree);
    }

    #[test]
    fn flow_insert_into_empty_under_key() {
        let mut tree = crate::parse("m: {}\n").unwrap();
        let map = flow_map_node(&tree);
        insert_map_entry(&mut tree, &map, 0, "a", Yaml::Int(1));
        assert_eq!(tree.emit(), "m: {a: 1}\n");
        reparse(&tree);
    }

    // ---- flow: remove ----

    #[test]
    fn flow_remove_first_with_spaces() {
        let mut tree = crate::parse("{a: 1, b: 2, c: 3}\n").unwrap();
        let map = flow_map_node(&tree);
        remove_map_entry(&mut tree, &map, 0);
        assert_eq!(tree.emit(), "{b: 2, c: 3}\n");
        reparse(&tree);
    }

    #[test]
    fn flow_remove_middle_with_spaces() {
        let mut tree = crate::parse("{a: 1, b: 2, c: 3}\n").unwrap();
        let map = flow_map_node(&tree);
        remove_map_entry(&mut tree, &map, 1);
        assert_eq!(tree.emit(), "{a: 1, c: 3}\n");
        reparse(&tree);
    }

    #[test]
    fn flow_remove_last_with_spaces() {
        let mut tree = crate::parse("{a: 1, b: 2, c: 3}\n").unwrap();
        let map = flow_map_node(&tree);
        remove_map_entry(&mut tree, &map, 2);
        assert_eq!(tree.emit(), "{a: 1, b: 2}\n");
        reparse(&tree);
    }

    #[test]
    fn flow_remove_tight() {
        let mut tree = crate::parse("{a: 1,b: 2,c: 3}\n").unwrap();
        let map = flow_map_node(&tree);
        remove_map_entry(&mut tree, &map, 0);
        assert_eq!(tree.emit(), "{b: 2,c: 3}\n");
        reparse(&tree);
    }

    #[test]
    fn flow_remove_only_entry_leaves_empty() {
        let mut tree = crate::parse("{a: 1}\n").unwrap();
        let map = flow_map_node(&tree);
        remove_map_entry(&mut tree, &map, 0);
        assert_eq!(tree.emit(), "{}\n");
        reparse(&tree);
    }

    #[test]
    fn flow_remove_out_of_range_is_noop() {
        let mut tree = crate::parse("{a: 1, b: 2}\n").unwrap();
        let map = flow_map_node(&tree);
        remove_map_entry(&mut tree, &map, 5);
        assert_eq!(tree.emit(), "{a: 1, b: 2}\n");
    }

    // ---- flow: hostile keys/values quote correctly (B3) ----

    /// Insert a `key: value` into a `{x: 0}` flow map and assert it
    /// reparses and decodes back to the expected map exactly.
    fn assert_flow_insert_round_trips(key: &str, value: Yaml) {
        let mut tree = crate::parse("{x: 0}\n").unwrap();
        let map = flow_map_node(&tree);
        insert_map_entry(&mut tree, &map, 99, key, value.clone());
        reparse(&tree);
        let v = Yaml::from_tree(&tree).unwrap();
        assert_eq!(
            v,
            Yaml::Map(vec![("x".into(), Yaml::Int(0)), (key.to_string(), value)])
        );
    }

    #[test]
    fn flow_insert_hostile_key_colon_space() {
        assert_flow_insert_round_trips("a: b", Yaml::Int(1));
    }

    #[test]
    fn flow_insert_hostile_key_and_value_comma() {
        assert_flow_insert_round_trips("k,1", Yaml::Str("v,2".into()));
    }

    #[test]
    fn flow_insert_value_with_close_brace() {
        assert_flow_insert_round_trips("k", Yaml::Str("a}b".into()));
    }

    #[test]
    fn flow_insert_nested_map_value_reads_back() {
        assert_flow_insert_round_trips("m", Yaml::Map(vec![("a: b".into(), Yaml::Int(1))]));
    }

    // ---- value fidelity ----

    #[test]
    fn block_inserted_value_reads_back() {
        let mut tree = crate::parse("a: 1\n").unwrap();
        let map = map_node(&tree);
        insert_map_entry(&mut tree, &map, 99, "b", Yaml::Int(2));
        let v = Yaml::from_tree(&tree).unwrap();
        assert_eq!(
            v,
            Yaml::Map(vec![("a".into(), Yaml::Int(1)), ("b".into(), Yaml::Int(2))])
        );
    }
}
