//! Structural entry insertion/removal for block collections.
//!
//! These are the single shared primitives for adding or dropping an
//! entry, owning the trivia/indent fix-up once so callers (builder,
//! fixers, comment placement, the scripting cursor) don't each
//! reinvent it. Both operate at the green-node level and apply via
//! `TreeEdit::ReplaceNode` on the collection.
//!
//! Fragments are produced through the value-model builder (strategy A):
//! we serialize the entry to canonical YAML, parse it, and splice the
//! resulting green subtree — so an inserted entry is always parse- and
//! round-trip-consistent.

use rowan::NodeOrToken;

use crate::SyntaxTree;
use crate::builder::Yaml;
use crate::edit::{GreenElement, TreeEdit, apply_edit, mk_token, splice_children_green};
use crate::syntax::tree_utils::observed_block_col;
use crate::syntax::{SyntaxKind, SyntaxNode};

/// Insert `key: value` into the block mapping `map` at entry position
/// `index` (clamped to the entry count). The new entry is indented to
/// the mapping's column and separated from its neighbours by a newline.
///
/// `map` must be a `BLOCK_MAPPING` node belonging to `tree`.
pub fn insert_map_entry(
    tree: &mut SyntaxTree,
    map: &SyntaxNode,
    index: usize,
    key: &str,
    value: Yaml,
) {
    debug_assert_eq!(map.kind(), SyntaxKind::BLOCK_MAPPING);
    let col = mapping_col(map);
    let entry_green = build_entry_green(key, value, col);

    // Find the child indices of existing entries so we can place the new
    // entry between them with a NEWLINE separator.
    let entry_child_indices: Vec<usize> = map
        .children_with_tokens()
        .enumerate()
        .filter(|(_, el)| {
            matches!(el, NodeOrToken::Node(n) if n.kind() == SyntaxKind::BLOCK_MAPPING_ENTRY)
        })
        .map(|(i, _)| i)
        .collect();

    let indent_ws: Vec<GreenElement> = if col > 0 {
        vec![NodeOrToken::Token(mk_token(
            SyntaxKind::WHITESPACE,
            &" ".repeat(col),
        ))]
    } else {
        Vec::new()
    };

    let (splice_at, fragment): (usize, Vec<GreenElement>) = if index >= entry_child_indices.len() {
        // Append after the last entry: NEWLINE + indent + entry.
        let at = map.children_with_tokens().count();
        let mut frag = vec![NodeOrToken::Token(mk_token(SyntaxKind::NEWLINE, "\n"))];
        frag.extend(indent_ws);
        frag.push(NodeOrToken::Node(entry_green));
        (at, frag)
    } else {
        // Insert before the entry at `index`: indent + entry + NEWLINE.
        let at = entry_child_indices[index];
        let mut frag = indent_ws;
        frag.push(NodeOrToken::Node(entry_green));
        frag.push(NodeOrToken::Token(mk_token(SyntaxKind::NEWLINE, "\n")));
        (at, frag)
    };

    let new_green = splice_children_green(map, splice_at..splice_at, fragment);
    apply_edit(
        tree,
        TreeEdit::ReplaceNode {
            target: map.clone(),
            new: new_green,
        },
    );
}

/// Remove the entry at position `index` from block mapping `map`,
/// together with the adjacent NEWLINE separator that joins it to the
/// rest, so no blank line is left behind. No-op when out of range.
pub fn remove_map_entry(tree: &mut SyntaxTree, map: &SyntaxNode, index: usize) {
    debug_assert_eq!(map.kind(), SyntaxKind::BLOCK_MAPPING);
    let children: Vec<_> = map.children_with_tokens().collect();
    let entry_positions: Vec<usize> = children
        .iter()
        .enumerate()
        .filter(|(_, el)| {
            matches!(el, NodeOrToken::Node(n) if n.kind() == SyntaxKind::BLOCK_MAPPING_ENTRY)
        })
        .map(|(i, _)| i)
        .collect();
    let Some(&entry_idx) = entry_positions.get(index) else {
        return;
    };

    // Widen the removed range to swallow one adjacent NEWLINE separator
    // (prefer the trailing one; fall back to the leading one for the
    // last entry) and any leading indent WHITESPACE that lived as a
    // sibling before the entry.
    let mut start = entry_idx;
    while start > 0 {
        match &children[start - 1] {
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::WHITESPACE => start -= 1,
            _ => break,
        }
    }
    let mut end = entry_idx + 1;
    if matches!(children.get(end), Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::NEWLINE) {
        end += 1;
    } else if start > 0
        && matches!(children.get(start - 1), Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::NEWLINE)
    {
        start -= 1;
    }

    let new_green = splice_children_green(map, start..end, Vec::new());
    apply_edit(
        tree,
        TreeEdit::ReplaceNode {
            target: map.clone(),
            new: new_green,
        },
    );
}

/// The column at which `map`'s entries are indented. Because entries
/// own their leading indent, the collection node itself reports column 0
/// via `observed_block_col`; so read the indent from the first entry's
/// own leading WHITESPACE, falling back to the observed column when the
/// first entry is at col 0 or begins mid-line.
fn mapping_col(map: &SyntaxNode) -> usize {
    for entry in map.children() {
        if entry.kind() != SyntaxKind::BLOCK_MAPPING_ENTRY {
            continue;
        }
        return match entry.children_with_tokens().next() {
            Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::WHITESPACE => t.text().len(),
            _ => observed_block_col(map),
        };
    }
    observed_block_col(map)
}

/// Build the green `BLOCK_MAPPING_ENTRY` for `key: value`, indented for
/// a mapping at column `col`. We serialize a single-entry mapping,
/// parse it, and lift its entry subtree — guaranteeing structural and
/// round-trip consistency with the parser.
fn build_entry_green(key: &str, value: Yaml, col: usize) -> rowan::GreenNode {
    let fragment = Yaml::Map(vec![(key.to_string(), value)]);
    // Build at column 0, then the splice prepends the indent WHITESPACE
    // as a sibling for nested entries; the entry's own first-line indent
    // is handled by the caller's `indent_ws`. Nested children inside the
    // value are already indented relative to col 0, so shift them.
    let text = indent_fragment(&fragment.to_yaml_string(), col);
    let tree = crate::parse(&text).expect("builder fragment must parse");
    tree.root()
        .descendants()
        .find(|n| n.kind() == SyntaxKind::BLOCK_MAPPING_ENTRY)
        .expect("fragment has an entry")
        .green()
        .into_owned()
}

/// Re-indent every line of `text` after the first by `col` spaces, so a
/// fragment built at column 0 nests correctly under a mapping at `col`.
/// The first line is left bare (the caller supplies its leading indent
/// as a sibling WHITESPACE token).
fn indent_fragment(text: &str, col: usize) -> String {
    if col == 0 {
        return text.to_string();
    }
    let pad = " ".repeat(col);
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split_inclusive('\n').enumerate() {
        if i > 0 && !line.trim_end_matches('\n').is_empty() {
            out.push_str(&pad);
        }
        out.push_str(line);
    }
    out
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

    #[test]
    fn insert_at_end() {
        let mut tree = crate::parse("a: 1\nb: 2\n").unwrap();
        let map = map_node(&tree);
        insert_map_entry(&mut tree, &map, 99, "c", Yaml::Int(3));
        assert_eq!(tree.emit(), "a: 1\nb: 2\nc: 3\n");
        crate::parse(&tree.emit()).expect("reparse");
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
        crate::parse(&tree.emit()).expect("reparse");
    }

    #[test]
    fn insert_into_nested_mapping_indents() {
        let mut tree = crate::parse("outer:\n  a: 1\n").unwrap();
        // Target the inner mapping.
        let inner = tree
            .root()
            .descendants()
            .filter(|n| n.kind() == SyntaxKind::BLOCK_MAPPING)
            .find(|n| mapping_col(n) == 2)
            .expect("inner map");
        insert_map_entry(&mut tree, &inner, 99, "b", Yaml::Int(2));
        assert_eq!(tree.emit(), "outer:\n  a: 1\n  b: 2\n");
        crate::parse(&tree.emit()).expect("reparse");
    }

    #[test]
    fn remove_middle_entry() {
        let mut tree = crate::parse("a: 1\nb: 2\nc: 3\n").unwrap();
        let map = map_node(&tree);
        remove_map_entry(&mut tree, &map, 1);
        assert_eq!(tree.emit(), "a: 1\nc: 3\n");
        crate::parse(&tree.emit()).expect("reparse");
    }

    #[test]
    fn remove_last_entry() {
        let mut tree = crate::parse("a: 1\nb: 2\n").unwrap();
        let map = map_node(&tree);
        remove_map_entry(&mut tree, &map, 1);
        assert_eq!(tree.emit(), "a: 1\n");
        crate::parse(&tree.emit()).expect("reparse");
    }

    #[test]
    fn remove_out_of_range_is_noop() {
        let mut tree = crate::parse("a: 1\n").unwrap();
        let map = map_node(&tree);
        remove_map_entry(&mut tree, &map, 5);
        assert_eq!(tree.emit(), "a: 1\n");
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
}
