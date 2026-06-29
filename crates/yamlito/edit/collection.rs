//! Shared primitives for editing block collections (mappings and
//! sequences).
//!
//! `entry.rs` (mappings) and `seq.rs` (sequences) were near-copy-paste
//! for the structural bookkeeping: collecting the entry child indices,
//! computing the indent column, assembling the append/insert fragment,
//! widening the removed range over a separator, and — when the last
//! entry is removed — collapsing the now-empty block node to its flow
//! empty form (`{}` / `[]`). This module owns that logic once,
//! parameterized by the entry `SyntaxKind` (`BLOCK_MAPPING_ENTRY` vs
//! `BLOCK_SEQUENCE_ENTRY`) and, for the collapse, by the flow brackets.
//!
//! The public `insert_map_entry` / `remove_map_entry` /
//! `insert_seq_item` / `remove_seq_item` fns remain the API surface;
//! they delegate the block layout here and keep only the kind-specific
//! fragment-building (a `key: value` entry vs a `- value` item) and the
//! flow-collection handling local.

use rowan::NodeOrToken;

use crate::SyntaxTree;
use crate::edit::{GreenElement, TreeEdit, apply_edit, mk_node, mk_token, splice_children_green};
use crate::syntax::tree_utils::observed_block_col;
use crate::syntax::{SyntaxKind, SyntaxNode};

/// Apply a `ReplaceNode { target, new }` edit on `target` and return the
/// *live* replacement node afterwards.
///
/// `ReplaceNode` splices `new` into `target`'s parent at `target`'s
/// child index (or makes it the tree root when `target` is parentless),
/// detaching the old `target` handle. The live node is then the child at
/// that same index in the (new) parent, or the new tree root. Both
/// `target` and `new` keep the same `SyntaxKind`.
pub(crate) fn replace_node_live(
    tree: &mut SyntaxTree,
    target: &SyntaxNode,
    new: rowan::GreenNode,
) -> SyntaxNode {
    // Record the target's position *before* the edit: the tree is
    // rebuilt immutably, so every handle taken now — `target` and its
    // ancestors alike — points into the old root afterwards. The
    // child-index path is what survives, and `ReplaceNode` is 1-for-1,
    // so the replacement lands at exactly the same path.
    let path = crate::edit::index_path(target);
    apply_edit(
        tree,
        TreeEdit::ReplaceNode {
            target: target.clone(),
            new,
        },
    );
    crate::edit::node_at_path(tree.root(), &path)
}

/// Child indices (within `node`) of the entries of kind `entry_kind`.
pub(crate) fn entry_child_indices(node: &SyntaxNode, entry_kind: SyntaxKind) -> Vec<usize> {
    node.children_with_tokens()
        .enumerate()
        .filter(|(_, el)| matches!(el, NodeOrToken::Node(n) if n.kind() == entry_kind))
        .map(|(i, _)| i)
        .collect()
}

/// The column at which `node`'s entries are indented. Because entries
/// own their leading indent, the collection node itself reports column 0
/// via `observed_block_col`; so read the indent from the first entry's
/// own leading WHITESPACE, falling back to the observed column when the
/// first entry is at col 0 or begins mid-line.
pub(crate) fn collection_col(node: &SyntaxNode, entry_kind: SyntaxKind) -> usize {
    for entry in node.children() {
        if entry.kind() != entry_kind {
            continue;
        }
        return match entry.children_with_tokens().next() {
            Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::WHITESPACE => t.text().len(),
            _ => observed_block_col(node),
        };
    }
    observed_block_col(node)
}

/// Insert an already-built entry green into a block collection `node` at
/// entry position `index` (clamped to the entry count), indenting it to
/// `col` and joining it to its neighbours with a NEWLINE. Returns the
/// live collection node after the edit.
pub(crate) fn block_insert(
    tree: &mut SyntaxTree,
    node: &SyntaxNode,
    index: usize,
    entry_kind: SyntaxKind,
    col: usize,
    entry_green: rowan::GreenNode,
) -> SyntaxNode {
    let entry_indices = entry_child_indices(node, entry_kind);

    let indent_ws: Vec<GreenElement> = if col > 0 {
        vec![NodeOrToken::Token(mk_token(
            SyntaxKind::WHITESPACE,
            &" ".repeat(col),
        ))]
    } else {
        Vec::new()
    };

    let (splice_at, fragment): (usize, Vec<GreenElement>) = if index >= entry_indices.len() {
        // Append after the last entry: NEWLINE + indent + entry.
        let at = node.children_with_tokens().count();
        let mut frag = vec![NodeOrToken::Token(mk_token(SyntaxKind::NEWLINE, "\n"))];
        frag.extend(indent_ws);
        frag.push(NodeOrToken::Node(entry_green));
        (at, frag)
    } else {
        // Insert before the entry at `index`: indent + entry + NEWLINE.
        let at = entry_indices[index];
        let mut frag = indent_ws;
        frag.push(NodeOrToken::Node(entry_green));
        frag.push(NodeOrToken::Token(mk_token(SyntaxKind::NEWLINE, "\n")));
        (at, frag)
    };

    let new_green = splice_children_green(node, splice_at..splice_at, fragment);
    replace_node_live(tree, node, new_green)
}

/// Remove the entry at position `index` from block collection `node`,
/// swallowing the adjacent NEWLINE separator (and any leading indent
/// WHITESPACE) so no blank line is left behind. No-op (returns `node`
/// unchanged) when out of range.
///
/// Removing the only remaining entry collapses the whole block node to
/// its flow empty form via `collapse`: a block collection cannot be
/// empty in YAML text (only `{}` / `[]` can), so an emptied block node is
/// unrepresentable and must convert.
pub(crate) fn block_remove(
    tree: &mut SyntaxTree,
    node: &SyntaxNode,
    index: usize,
    entry_kind: SyntaxKind,
    collapse: impl FnOnce(&mut SyntaxTree, &SyntaxNode) -> SyntaxNode,
) -> SyntaxNode {
    let children: Vec<_> = node.children_with_tokens().collect();
    let entry_positions: Vec<usize> = children
        .iter()
        .enumerate()
        .filter(|(_, el)| matches!(el, NodeOrToken::Node(n) if n.kind() == entry_kind))
        .map(|(i, _)| i)
        .collect();
    let Some(&entry_idx) = entry_positions.get(index) else {
        return node.clone();
    };

    // Removing the only remaining entry: collapse to the flow empty form.
    if entry_positions.len() == 1 {
        return collapse(tree, node);
    }

    // Widen the removed range to swallow one adjacent NEWLINE separator
    // (prefer the trailing one; fall back to the leading one for the
    // last entry) and any leading indent WHITESPACE sibling.
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

    let new_green = splice_children_green(node, start..end, Vec::new());
    replace_node_live(tree, node, new_green)
}

/// Replace a now-empty block collection with its flow empty form
/// (`flow_kind` wrapping `open`/`close` bracket tokens). Because the flow
/// node has a different `SyntaxKind` than the block one, the swap can't
/// go through `ReplaceNode` on the collection itself (kinds must match);
/// instead we splice on the parent.
///
/// When the collection is a mapping value laid out on its own line
/// (`key:\n  - a` / `key:\n  a: 1`), the intervening NEWLINE (and any
/// indent) between the `:` and the collection is replaced with a single
/// space so the result reads `key: []` / `key: {}`. A document-root
/// collection (no preceding COLON) is replaced in place, yielding a bare
/// `[]` / `{}`.
pub(crate) fn collapse_block_collection_to_empty(
    tree: &mut SyntaxTree,
    node: &SyntaxNode,
    flow_kind: SyntaxKind,
    open: (SyntaxKind, &str),
    close: (SyntaxKind, &str),
) -> SyntaxNode {
    let empty = NodeOrToken::Node(mk_node(
        flow_kind,
        [
            NodeOrToken::Token(mk_token(open.0, open.1)),
            NodeOrToken::Token(mk_token(close.0, close.1)),
        ],
    ));

    let Some(parent) = node.parent() else {
        // Parentless root collection: rebuild the tree's whole root from
        // the emitted empty form reparsed. Rare; keep it simple.
        let text = format!("{}{}\n", open.1, close.1);
        let rebuilt = crate::parse(&text).expect("empty flow form parses");
        tree.set_root(rebuilt.root().clone());
        return tree
            .root()
            .descendants()
            .find(|n| n.kind() == flow_kind)
            .expect("rebuilt empty form has a flow collection");
    };

    let siblings: Vec<_> = parent.children_with_tokens().collect();
    let node_idx = node.index();

    // Walk back over the trivia (NEWLINE / WHITESPACE) immediately before
    // the collection, stopping at the first non-trivia element.
    let mut start = node_idx;
    let mut saw_newline = false;
    while start > 0 {
        match &siblings[start - 1] {
            NodeOrToken::Token(t)
                if matches!(t.kind(), SyntaxKind::NEWLINE | SyntaxKind::WHITESPACE) =>
            {
                if t.kind() == SyntaxKind::NEWLINE {
                    saw_newline = true;
                }
                start -= 1;
            }
            _ => break,
        }
    }

    // If a COLON precedes the (collapsed) trivia, this is a mapping value
    // on its own line — bring it back onto the key line with one space.
    let preceded_by_colon = start > 0
        && matches!(&siblings[start - 1], NodeOrToken::Token(t) if t.kind() == SyntaxKind::COLON);

    let mut fragment: Vec<GreenElement> = Vec::new();
    if saw_newline && preceded_by_colon {
        fragment.push(NodeOrToken::Token(mk_token(SyntaxKind::WHITESPACE, " ")));
    }
    fragment.push(empty);
    // The flow empty node is the last element of the fragment; after the
    // splice it lands at child-index `start + (fragment.len() - 1)` in the
    // new parent (any leading WHITESPACE occupies the slots before it).
    let flow_child_idx = start + (fragment.len() - 1);

    let new_parent_green = splice_children_green(&parent, start..node_idx + 1, fragment);
    let new_parent = replace_node_live(tree, &parent, new_parent_green);
    match new_parent.children_with_tokens().nth(flow_child_idx) {
        Some(NodeOrToken::Node(n)) if n.kind() == flow_kind => n,
        _ => unreachable!("collapsed empty flow node sits at the spliced index"),
    }
}
