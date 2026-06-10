//! Structural item insertion/removal for sequences (block and flow).
//!
//! The sequence counterpart to `entry.rs`: the single shared primitives
//! for adding or dropping a sequence item, owning the
//! separator/indent/bracket fix-up once so callers (builder, fixers, the
//! scripting cursor) don't each reinvent it. Both operate at the
//! green-node level and apply via `TreeEdit::ReplaceNode` on the
//! sequence.
//!
//! Two layouts are handled uniformly behind one entry point:
//!
//! - **Block** (`- a\n- b`): items are `BLOCK_SEQUENCE_ENTRY` nodes
//!   separated by NEWLINE siblings; line-start items own their leading
//!   indent WHITESPACE. Insert/remove mirror `insert_map_entry` /
//!   `remove_map_entry` exactly.
//! - **Flow** (`[a, b]`): items are `FLOW_SEQUENCE_ENTRY` nodes between
//!   `[`/`]`, separated by `,` plus optional WHITESPACE. We sample the
//!   existing separator so `[a,b]` stays tight and `[a, b]` stays
//!   spaced.
//!
//! A block sequence can never be empty (an empty list is only spelled
//! `[]` in flow), so removing the *last* block item converts the whole
//! `BLOCK_SEQUENCE` into a flow `[]`, keeping the tree valid and
//! round-trippable. Conversely, pushing into that `[]` grows the flow
//! sequence.
//!
//! Item fragments are produced through the value-model builder
//! (strategy A): the value is serialized to canonical YAML, parsed, and
//! the resulting green subtree spliced — so an inserted item is always
//! parse- and round-trip-consistent.

use rowan::NodeOrToken;

use crate::SyntaxTree;
use crate::builder::Yaml;
use crate::edit::{GreenElement, TreeEdit, apply_edit, mk_node, mk_token, splice_children_green};
use crate::syntax::tree_utils::observed_block_col;
use crate::syntax::{SyntaxKind, SyntaxNode};

/// Insert `value` into the sequence `seq` at item position `index`
/// (clamped to the item count). Dispatches on the node kind: a
/// `BLOCK_SEQUENCE` gets a dash item on its own line; a `FLOW_SEQUENCE`
/// gets a `[...]` item with separator spacing matching its neighbours.
///
/// `seq` must be a `BLOCK_SEQUENCE` or `FLOW_SEQUENCE` belonging to
/// `tree`.
pub fn insert_seq_item(tree: &mut SyntaxTree, seq: &SyntaxNode, index: usize, value: Yaml) {
    match seq.kind() {
        SyntaxKind::BLOCK_SEQUENCE => insert_block_item(tree, seq, index, value),
        SyntaxKind::FLOW_SEQUENCE => insert_flow_item(tree, seq, index, value),
        other => debug_assert!(false, "insert_seq_item: not a sequence: {other:?}"),
    }
}

/// Append `value` after the last item of `seq`. Convenience for
/// `insert_seq_item(tree, seq, usize::MAX, value)`.
pub fn push_seq_item(tree: &mut SyntaxTree, seq: &SyntaxNode, value: Yaml) {
    insert_seq_item(tree, seq, usize::MAX, value);
}

/// Remove the item at position `index` from `seq`, together with the
/// adjacent separator (NEWLINE for block, `,`/WHITESPACE for flow) that
/// joins it to the rest, so no stray separator is left behind. No-op
/// when out of range.
///
/// Removing the last remaining item of a *block* sequence converts the
/// whole node into an empty flow `[]` (a block sequence cannot be
/// empty); removing the last item of a flow sequence leaves `[]`.
pub fn remove_seq_item(tree: &mut SyntaxTree, seq: &SyntaxNode, index: usize) {
    match seq.kind() {
        SyntaxKind::BLOCK_SEQUENCE => remove_block_item(tree, seq, index),
        SyntaxKind::FLOW_SEQUENCE => remove_flow_item(tree, seq, index),
        other => debug_assert!(false, "remove_seq_item: not a sequence: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Block sequences
// ---------------------------------------------------------------------------

fn item_child_indices(seq: &SyntaxNode, kind: SyntaxKind) -> Vec<usize> {
    seq.children_with_tokens()
        .enumerate()
        .filter(|(_, el)| matches!(el, NodeOrToken::Node(n) if n.kind() == kind))
        .map(|(i, _)| i)
        .collect()
}

fn insert_block_item(tree: &mut SyntaxTree, seq: &SyntaxNode, index: usize, value: Yaml) {
    let col = sequence_col(seq);
    let item_green = build_block_item_green(value, col);

    let item_indices = item_child_indices(seq, SyntaxKind::BLOCK_SEQUENCE_ENTRY);

    let indent_ws: Vec<GreenElement> = if col > 0 {
        vec![NodeOrToken::Token(mk_token(
            SyntaxKind::WHITESPACE,
            &" ".repeat(col),
        ))]
    } else {
        Vec::new()
    };

    let (splice_at, fragment): (usize, Vec<GreenElement>) = if index >= item_indices.len() {
        // Append after the last item: NEWLINE + indent + item.
        let at = seq.children_with_tokens().count();
        let mut frag = vec![NodeOrToken::Token(mk_token(SyntaxKind::NEWLINE, "\n"))];
        frag.extend(indent_ws);
        frag.push(NodeOrToken::Node(item_green));
        (at, frag)
    } else {
        // Insert before the item at `index`: indent + item + NEWLINE.
        let at = item_indices[index];
        let mut frag = indent_ws;
        frag.push(NodeOrToken::Node(item_green));
        frag.push(NodeOrToken::Token(mk_token(SyntaxKind::NEWLINE, "\n")));
        (at, frag)
    };

    let new_green = splice_children_green(seq, splice_at..splice_at, fragment);
    apply_edit(
        tree,
        TreeEdit::ReplaceNode {
            target: seq.clone(),
            new: new_green,
        },
    );
}

fn remove_block_item(tree: &mut SyntaxTree, seq: &SyntaxNode, index: usize) {
    let children: Vec<_> = seq.children_with_tokens().collect();
    let item_positions: Vec<usize> = children
        .iter()
        .enumerate()
        .filter(|(_, el)| {
            matches!(el, NodeOrToken::Node(n) if n.kind() == SyntaxKind::BLOCK_SEQUENCE_ENTRY)
        })
        .map(|(i, _)| i)
        .collect();
    let Some(&item_idx) = item_positions.get(index) else {
        return;
    };

    // Removing the only remaining item: a block sequence cannot be empty,
    // so collapse the whole node to a flow `[]`.
    if item_positions.len() == 1 {
        collapse_block_seq_to_empty(tree, seq);
        return;
    }

    // Widen the removed range to swallow one adjacent NEWLINE separator
    // (prefer the trailing one; fall back to the leading one for the last
    // item) plus any leading indent WHITESPACE sibling.
    let mut start = item_idx;
    while start > 0 {
        match &children[start - 1] {
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::WHITESPACE => start -= 1,
            _ => break,
        }
    }
    let mut end = item_idx + 1;
    if matches!(children.get(end), Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::NEWLINE) {
        end += 1;
    } else if start > 0
        && matches!(children.get(start - 1), Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::NEWLINE)
    {
        start -= 1;
    }

    let new_green = splice_children_green(seq, start..end, Vec::new());
    apply_edit(
        tree,
        TreeEdit::ReplaceNode {
            target: seq.clone(),
            new: new_green,
        },
    );
}

/// Replace a now-empty block sequence with a flow `[]`. Because a flow
/// node has a different `SyntaxKind` than the block one, the swap can't
/// go through `ReplaceNode` on the sequence itself (kinds must match);
/// instead we splice on the parent.
///
/// When the sequence is a mapping value laid out on its own line
/// (`key:\n  - a`), the intervening `NEWLINE` (and any indent) between
/// the `:` and the sequence is replaced with a single space so the
/// result reads `key: []`. A document-root sequence (no preceding
/// `COLON`) is replaced in place, yielding bare `[]`.
fn collapse_block_seq_to_empty(tree: &mut SyntaxTree, seq: &SyntaxNode) {
    let empty = NodeOrToken::Node(mk_node(
        SyntaxKind::FLOW_SEQUENCE,
        [
            NodeOrToken::Token(mk_token(SyntaxKind::L_BRACKET, "[")),
            NodeOrToken::Token(mk_token(SyntaxKind::R_BRACKET, "]")),
        ],
    ));

    let Some(parent) = seq.parent() else {
        // Parentless root sequence: swap the tree's whole root for the
        // emitted `[]` reparsed. Rare; keep it simple and correct.
        let rebuilt = crate::parse("[]\n").expect("`[]` parses");
        tree.set_root(rebuilt.root().clone());
        return;
    };

    let siblings: Vec<_> = parent.children_with_tokens().collect();
    let seq_idx = seq.index();

    // Walk back over the trivia (NEWLINE / WHITESPACE) immediately before
    // the sequence, stopping at the first non-trivia element.
    let mut start = seq_idx;
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

    let new_parent = splice_children_green(&parent, start..seq_idx + 1, fragment);
    apply_edit(
        tree,
        TreeEdit::ReplaceNode {
            target: parent,
            new: new_parent,
        },
    );
}

/// The column at which `seq`'s dashes are indented. Line-start items own
/// their leading indent WHITESPACE, so the collection node reports
/// column 0 via `observed_block_col`; read the indent from the first
/// item's own leading WHITESPACE, falling back to the observed column.
fn sequence_col(seq: &SyntaxNode) -> usize {
    for item in seq.children() {
        if item.kind() != SyntaxKind::BLOCK_SEQUENCE_ENTRY {
            continue;
        }
        return match item.children_with_tokens().next() {
            Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::WHITESPACE => t.text().len(),
            _ => observed_block_col(seq),
        };
    }
    observed_block_col(seq)
}

/// Build the green `BLOCK_SEQUENCE_ENTRY` for `- value`, indented for a
/// sequence at column `col`. We serialize a single-item sequence, parse
/// it, and lift its entry subtree — guaranteeing structural and
/// round-trip consistency with the parser.
fn build_block_item_green(value: Yaml, col: usize) -> rowan::GreenNode {
    let fragment = Yaml::Seq(vec![value]);
    let text = indent_fragment(&fragment.to_yaml_string(), col);
    let tree = crate::parse(&text).expect("builder fragment must parse");
    tree.root()
        .descendants()
        .find(|n| n.kind() == SyntaxKind::BLOCK_SEQUENCE_ENTRY)
        .expect("fragment has a sequence entry")
        .green()
        .into_owned()
}

/// Re-indent every line of `text` after the first by `col` spaces, so a
/// fragment built at column 0 nests correctly under a sequence at `col`.
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

// ---------------------------------------------------------------------------
// Flow sequences
// ---------------------------------------------------------------------------

/// Build the green `FLOW_SEQUENCE_ENTRY` for `value` rendered inline. We
/// serialize the value as a flow item, parse `[value]`, and lift the
/// entry subtree.
fn build_flow_item_green(value: Yaml) -> rowan::GreenNode {
    // Wrap in a single-item flow sequence so the value renders inline,
    // independent of its block-vs-flow shape.
    let text = format!("[{}]\n", flow_scalar_text(&value));
    let tree = crate::parse(&text).expect("flow item fragment must parse");
    tree.root()
        .descendants()
        .find(|n| n.kind() == SyntaxKind::FLOW_SEQUENCE_ENTRY)
        .expect("fragment has a flow sequence entry")
        .green()
        .into_owned()
}

/// Render `value` as inline flow text suitable for placing inside `[]`.
/// Scalars use the builder's canonical scalar form; nested collections
/// fall back to the builder's block form trimmed of its trailing
/// newline, which the parser then accepts as a flow item only for
/// scalars — collections inside flow need flow syntax, so we serialize
/// them via a one-element flow wrapper round-trip.
fn flow_scalar_text(value: &Yaml) -> String {
    // For scalars the canonical block string is already a valid inline
    // token (`true`, `42`, `'a b'`, ...). Collections are serialized as
    // flow by recursively building a flow string.
    match value {
        Yaml::Null | Yaml::Bool(_) | Yaml::Int(_) | Yaml::Float(_) | Yaml::Str(_) => {
            value.to_yaml_string().trim_end_matches('\n').to_string()
        }
        Yaml::Seq(items) => {
            let inner: Vec<String> = items.iter().map(flow_scalar_text).collect();
            format!("[{}]", inner.join(", "))
        }
        Yaml::Map(pairs) => {
            let inner: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", k, flow_scalar_text(v)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

/// Sample the comma-to-item separator used inside `seq`: whether a
/// WHITESPACE follows the comma (`[a, b]` vs `[a,b]`). Defaults to a
/// single space when the sequence has fewer than two items to observe.
fn flow_separator_ws(seq: &SyntaxNode) -> Option<String> {
    let children: Vec<_> = seq.children_with_tokens().collect();
    for (i, el) in children.iter().enumerate() {
        if matches!(el, NodeOrToken::Token(t) if t.kind() == SyntaxKind::COMMA) {
            // Look at what follows the comma.
            return match children.get(i + 1) {
                Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::WHITESPACE => {
                    Some(t.text().to_string())
                }
                // Multiline (comma then newline) or tight (comma then item):
                // report no inline whitespace; the caller decides.
                _ => None,
            };
        }
    }
    // No existing separator to sample.
    Some(" ".to_string())
}

fn insert_flow_item(tree: &mut SyntaxTree, seq: &SyntaxNode, index: usize, value: Yaml) {
    let item_green = build_flow_item_green(value);
    let item_indices = item_child_indices(seq, SyntaxKind::FLOW_SEQUENCE_ENTRY);
    let sep_ws = flow_separator_ws(seq);

    // Locate the closing bracket so an item dropped into an empty `[]`
    // lands just inside it.
    let children: Vec<_> = seq.children_with_tokens().collect();
    let r_bracket_idx = children
        .iter()
        .position(|el| matches!(el, NodeOrToken::Token(t) if t.kind() == SyntaxKind::R_BRACKET));

    let comma = || NodeOrToken::Token(mk_token(SyntaxKind::COMMA, ","));
    let ws = |s: &str| NodeOrToken::Token(mk_token(SyntaxKind::WHITESPACE, s));

    if item_indices.is_empty() {
        // Empty `[]` — drop the item just before `]` with no separator.
        let at = r_bracket_idx.unwrap_or(children.len());
        let frag = vec![NodeOrToken::Node(item_green)];
        let new_green = splice_children_green(seq, at..at, frag);
        apply_edit(
            tree,
            TreeEdit::ReplaceNode {
                target: seq.clone(),
                new: new_green,
            },
        );
        return;
    }

    let (splice_at, fragment): (usize, Vec<GreenElement>) = if index >= item_indices.len() {
        // Append after the last item: `, item` (matching observed sep).
        let last = *item_indices.last().unwrap();
        let at = last + 1;
        let mut frag = vec![comma()];
        if let Some(s) = &sep_ws {
            frag.push(ws(s));
        }
        frag.push(NodeOrToken::Node(item_green));
        (at, frag)
    } else {
        // Insert before the item at `index`: `item,<sep>` so the existing
        // item keeps its position. Any whitespace currently sitting before
        // `at` belongs to the old item's lead-in and stays put.
        let at = item_indices[index];
        let mut frag = vec![NodeOrToken::Node(item_green), comma()];
        if let Some(s) = &sep_ws {
            frag.push(ws(s));
        }
        (at, frag)
    };

    let new_green = splice_children_green(seq, splice_at..splice_at, fragment);
    apply_edit(
        tree,
        TreeEdit::ReplaceNode {
            target: seq.clone(),
            new: new_green,
        },
    );
}

fn remove_flow_item(tree: &mut SyntaxTree, seq: &SyntaxNode, index: usize) {
    let children: Vec<_> = seq.children_with_tokens().collect();
    let item_positions: Vec<usize> = children
        .iter()
        .enumerate()
        .filter(|(_, el)| {
            matches!(el, NodeOrToken::Node(n) if n.kind() == SyntaxKind::FLOW_SEQUENCE_ENTRY)
        })
        .map(|(i, _)| i)
        .collect();
    let Some(&item_idx) = item_positions.get(index) else {
        return;
    };

    let is_comma = |i: usize| {
        matches!(children.get(i), Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::COMMA)
    };
    let is_ws = |i: usize| {
        matches!(children.get(i), Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::WHITESPACE)
    };

    let mut start = item_idx;
    let mut end = item_idx + 1;

    if index + 1 < item_positions.len() {
        // Not the last item: swallow the following `,` and any WHITESPACE
        // up to (but not including) the next item, so the successor keeps
        // its position.
        if is_comma(end) {
            end += 1;
        }
        while is_ws(end) {
            end += 1;
        }
    } else {
        // Last (but not only) item: swallow the *preceding* `,` and the
        // whitespace around it, so no dangling comma remains.
        if is_ws(start.saturating_sub(1)) && start > 0 {
            start -= 1;
        }
        if start > 0 && is_comma(start - 1) {
            start -= 1;
        }
        // Also drop whitespace before that comma (rare, e.g. `[a , b]`).
        while start > 0 && is_ws(start - 1) {
            start -= 1;
        }
    }

    let new_green = splice_children_green(seq, start..end, Vec::new());
    apply_edit(
        tree,
        TreeEdit::ReplaceNode {
            target: seq.clone(),
            new: new_green,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::SyntaxKind;

    /// Parse `src` and return its first BLOCK_SEQUENCE node.
    fn block_seq(tree: &SyntaxTree) -> SyntaxNode {
        tree.root()
            .descendants()
            .find(|n| n.kind() == SyntaxKind::BLOCK_SEQUENCE)
            .expect("block sequence")
    }

    /// Parse `src` and return its first FLOW_SEQUENCE node.
    fn flow_seq(tree: &SyntaxTree) -> SyntaxNode {
        tree.root()
            .descendants()
            .find(|n| n.kind() == SyntaxKind::FLOW_SEQUENCE)
            .expect("flow sequence")
    }

    fn reparse(tree: &SyntaxTree) {
        crate::parse(&tree.emit()).expect("emitted text must reparse");
    }

    // ---- block: push / append ----

    #[test]
    fn block_push_at_top_level() {
        let mut tree = crate::parse("- a\n- b\n").unwrap();
        let seq = block_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("c".into()));
        assert_eq!(tree.emit(), "- a\n- b\n- c\n");
        reparse(&tree);
    }

    #[test]
    fn block_push_no_trailing_newline() {
        // Last item has no trailing NEWLINE; append still works.
        let mut tree = crate::parse("- a\n- b").unwrap();
        let seq = block_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("c".into()));
        assert_eq!(tree.emit(), "- a\n- b\n- c");
        reparse(&tree);
    }

    #[test]
    fn block_push_into_single() {
        let mut tree = crate::parse("- a\n").unwrap();
        let seq = block_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("b".into()));
        assert_eq!(tree.emit(), "- a\n- b\n");
        reparse(&tree);
    }

    // ---- block: insert at index ----

    #[test]
    fn block_insert_at_front() {
        let mut tree = crate::parse("- b\n- c\n").unwrap();
        let seq = block_seq(&tree);
        insert_seq_item(&mut tree, &seq, 0, Yaml::Str("a".into()));
        assert_eq!(tree.emit(), "- a\n- b\n- c\n");
        reparse(&tree);
    }

    #[test]
    fn block_insert_in_middle() {
        let mut tree = crate::parse("- a\n- c\n").unwrap();
        let seq = block_seq(&tree);
        insert_seq_item(&mut tree, &seq, 1, Yaml::Str("b".into()));
        assert_eq!(tree.emit(), "- a\n- b\n- c\n");
        reparse(&tree);
    }

    #[test]
    fn block_insert_index_past_end_appends() {
        let mut tree = crate::parse("- a\n").unwrap();
        let seq = block_seq(&tree);
        insert_seq_item(&mut tree, &seq, 99, Yaml::Int(2));
        assert_eq!(tree.emit(), "- a\n- 2\n");
        reparse(&tree);
    }

    // ---- block: nested / indented ----

    #[test]
    fn block_push_into_nested_indented() {
        let mut tree = crate::parse("items:\n  - a\n  - b\n").unwrap();
        let seq = block_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("c".into()));
        assert_eq!(tree.emit(), "items:\n  - a\n  - b\n  - c\n");
        reparse(&tree);
    }

    #[test]
    fn block_insert_front_nested_indented() {
        let mut tree = crate::parse("items:\n  - b\n").unwrap();
        let seq = block_seq(&tree);
        insert_seq_item(&mut tree, &seq, 0, Yaml::Str("a".into()));
        assert_eq!(tree.emit(), "items:\n  - a\n  - b\n");
        reparse(&tree);
    }

    #[test]
    fn block_push_deeply_nested() {
        let mut tree = crate::parse("a:\n  b:\n    - x\n    - y\n").unwrap();
        let seq = block_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("z".into()));
        assert_eq!(tree.emit(), "a:\n  b:\n    - x\n    - y\n    - z\n");
        reparse(&tree);
    }

    #[test]
    fn block_push_map_item() {
        let mut tree = crate::parse("- k: 1\n").unwrap();
        let seq = block_seq(&tree);
        push_seq_item(
            &mut tree,
            &seq,
            Yaml::Map(vec![("k".into(), Yaml::Int(2))]),
        );
        assert_eq!(tree.emit(), "- k: 1\n- k: 2\n");
        reparse(&tree);
    }

    #[test]
    fn block_push_string_quoted_when_needed() {
        let mut tree = crate::parse("- a\n").unwrap();
        let seq = block_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("true".into()));
        assert_eq!(tree.emit(), "- a\n- 'true'\n");
        reparse(&tree);
    }

    // ---- block: remove ----

    #[test]
    fn block_remove_first() {
        let mut tree = crate::parse("- a\n- b\n- c\n").unwrap();
        let seq = block_seq(&tree);
        remove_seq_item(&mut tree, &seq, 0);
        assert_eq!(tree.emit(), "- b\n- c\n");
        reparse(&tree);
    }

    #[test]
    fn block_remove_middle() {
        let mut tree = crate::parse("- a\n- b\n- c\n").unwrap();
        let seq = block_seq(&tree);
        remove_seq_item(&mut tree, &seq, 1);
        assert_eq!(tree.emit(), "- a\n- c\n");
        reparse(&tree);
    }

    #[test]
    fn block_remove_last() {
        let mut tree = crate::parse("- a\n- b\n").unwrap();
        let seq = block_seq(&tree);
        remove_seq_item(&mut tree, &seq, 1);
        assert_eq!(tree.emit(), "- a\n");
        reparse(&tree);
    }

    #[test]
    fn block_remove_nested_indented() {
        let mut tree = crate::parse("items:\n  - a\n  - b\n  - c\n").unwrap();
        let seq = block_seq(&tree);
        remove_seq_item(&mut tree, &seq, 1);
        assert_eq!(tree.emit(), "items:\n  - a\n  - c\n");
        reparse(&tree);
    }

    #[test]
    fn block_remove_out_of_range_is_noop() {
        let mut tree = crate::parse("- a\n- b\n").unwrap();
        let seq = block_seq(&tree);
        remove_seq_item(&mut tree, &seq, 5);
        assert_eq!(tree.emit(), "- a\n- b\n");
    }

    // ---- block: emptied-by-removal collapses to flow `[]` ----

    #[test]
    fn block_remove_only_item_collapses_to_flow_empty() {
        let mut tree = crate::parse("items:\n  - a\n").unwrap();
        let seq = block_seq(&tree);
        remove_seq_item(&mut tree, &seq, 0);
        assert_eq!(tree.emit(), "items: []\n");
        reparse(&tree);
    }

    #[test]
    fn block_remove_down_to_empty_then_push() {
        let mut tree = crate::parse("items:\n  - a\n").unwrap();
        let seq = block_seq(&tree);
        remove_seq_item(&mut tree, &seq, 0);
        assert_eq!(tree.emit(), "items: []\n");
        // Now the value is a flow sequence; push grows it.
        let seq = flow_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("b".into()));
        assert_eq!(tree.emit(), "items: [b]\n");
        reparse(&tree);
    }

    // ---- flow: push / append, spacing variants ----

    #[test]
    fn flow_push_with_spaces() {
        let mut tree = crate::parse("[a, b]\n").unwrap();
        let seq = flow_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("c".into()));
        assert_eq!(tree.emit(), "[a, b, c]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_push_without_spaces() {
        let mut tree = crate::parse("[a,b]\n").unwrap();
        let seq = flow_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("c".into()));
        assert_eq!(tree.emit(), "[a,b,c]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_push_into_single() {
        let mut tree = crate::parse("[a]\n").unwrap();
        let seq = flow_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("b".into()));
        assert_eq!(tree.emit(), "[a, b]\n");
        reparse(&tree);
    }

    // ---- flow: insert at index ----

    #[test]
    fn flow_insert_at_front_with_spaces() {
        let mut tree = crate::parse("[b, c]\n").unwrap();
        let seq = flow_seq(&tree);
        insert_seq_item(&mut tree, &seq, 0, Yaml::Str("a".into()));
        assert_eq!(tree.emit(), "[a, b, c]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_insert_at_front_no_spaces() {
        let mut tree = crate::parse("[b,c]\n").unwrap();
        let seq = flow_seq(&tree);
        insert_seq_item(&mut tree, &seq, 0, Yaml::Str("a".into()));
        assert_eq!(tree.emit(), "[a,b,c]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_insert_in_middle() {
        let mut tree = crate::parse("[a, c]\n").unwrap();
        let seq = flow_seq(&tree);
        insert_seq_item(&mut tree, &seq, 1, Yaml::Str("b".into()));
        assert_eq!(tree.emit(), "[a, b, c]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_insert_index_past_end_appends() {
        let mut tree = crate::parse("[a]\n").unwrap();
        let seq = flow_seq(&tree);
        insert_seq_item(&mut tree, &seq, 99, Yaml::Int(2));
        assert_eq!(tree.emit(), "[a, 2]\n");
        reparse(&tree);
    }

    // ---- flow: empty list ----

    #[test]
    fn flow_push_into_empty() {
        let mut tree = crate::parse("[]\n").unwrap();
        let seq = flow_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("a".into()));
        assert_eq!(tree.emit(), "[a]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_insert_into_empty() {
        let mut tree = crate::parse("[]\n").unwrap();
        let seq = flow_seq(&tree);
        insert_seq_item(&mut tree, &seq, 0, Yaml::Str("a".into()));
        assert_eq!(tree.emit(), "[a]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_push_into_empty_under_key() {
        let mut tree = crate::parse("items: []\n").unwrap();
        let seq = flow_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Int(1));
        assert_eq!(tree.emit(), "items: [1]\n");
        reparse(&tree);
    }

    // ---- flow: remove ----

    #[test]
    fn flow_remove_first_with_spaces() {
        let mut tree = crate::parse("[a, b, c]\n").unwrap();
        let seq = flow_seq(&tree);
        remove_seq_item(&mut tree, &seq, 0);
        assert_eq!(tree.emit(), "[b, c]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_remove_middle_with_spaces() {
        let mut tree = crate::parse("[a, b, c]\n").unwrap();
        let seq = flow_seq(&tree);
        remove_seq_item(&mut tree, &seq, 1);
        assert_eq!(tree.emit(), "[a, c]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_remove_last_with_spaces() {
        let mut tree = crate::parse("[a, b, c]\n").unwrap();
        let seq = flow_seq(&tree);
        remove_seq_item(&mut tree, &seq, 2);
        assert_eq!(tree.emit(), "[a, b]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_remove_first_no_spaces() {
        let mut tree = crate::parse("[a,b,c]\n").unwrap();
        let seq = flow_seq(&tree);
        remove_seq_item(&mut tree, &seq, 0);
        assert_eq!(tree.emit(), "[b,c]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_remove_last_no_spaces() {
        let mut tree = crate::parse("[a,b,c]\n").unwrap();
        let seq = flow_seq(&tree);
        remove_seq_item(&mut tree, &seq, 2);
        assert_eq!(tree.emit(), "[a,b]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_remove_only_item_leaves_empty() {
        let mut tree = crate::parse("[a]\n").unwrap();
        let seq = flow_seq(&tree);
        remove_seq_item(&mut tree, &seq, 0);
        assert_eq!(tree.emit(), "[]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_remove_out_of_range_is_noop() {
        let mut tree = crate::parse("[a, b]\n").unwrap();
        let seq = flow_seq(&tree);
        remove_seq_item(&mut tree, &seq, 5);
        assert_eq!(tree.emit(), "[a, b]\n");
    }

    #[test]
    fn flow_remove_down_to_empty_then_push() {
        let mut tree = crate::parse("[a]\n").unwrap();
        let seq = flow_seq(&tree);
        remove_seq_item(&mut tree, &seq, 0);
        assert_eq!(tree.emit(), "[]\n");
        let seq = flow_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("b".into()));
        assert_eq!(tree.emit(), "[b]\n");
        reparse(&tree);
    }

    // ---- flow: multiline ----

    #[test]
    fn flow_push_multiline_samples_no_inline_ws() {
        // Multiline flow: separator is comma+newline, so we report no
        // inline whitespace and append `,c` after the last item.
        let mut tree = crate::parse("[\n  a,\n  b,\n]\n").unwrap();
        let seq = flow_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("c".into()));
        // The appended item lands tight after `b`; result still reparses.
        reparse(&tree);
        let v = Yaml::from_tree(&tree).unwrap();
        assert_eq!(
            v,
            Yaml::Seq(vec![
                Yaml::Str("a".into()),
                Yaml::Str("b".into()),
                Yaml::Str("c".into())
            ])
        );
    }

    #[test]
    fn flow_remove_multiline_first() {
        let mut tree = crate::parse("[\n  a,\n  b,\n]\n").unwrap();
        let seq = flow_seq(&tree);
        remove_seq_item(&mut tree, &seq, 0);
        reparse(&tree);
        let v = Yaml::from_tree(&tree).unwrap();
        assert_eq!(v, Yaml::Seq(vec![Yaml::Str("b".into())]));
    }

    // ---- value fidelity ----

    #[test]
    fn block_inserted_value_reads_back() {
        let mut tree = crate::parse("- 1\n").unwrap();
        let seq = block_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Int(2));
        let v = Yaml::from_tree(&tree).unwrap();
        assert_eq!(v, Yaml::Seq(vec![Yaml::Int(1), Yaml::Int(2)]));
    }

    #[test]
    fn flow_inserted_nested_seq_reads_back() {
        let mut tree = crate::parse("[a]\n").unwrap();
        let seq = flow_seq(&tree);
        push_seq_item(
            &mut tree,
            &seq,
            Yaml::Seq(vec![Yaml::Int(1), Yaml::Int(2)]),
        );
        reparse(&tree);
        let v = Yaml::from_tree(&tree).unwrap();
        assert_eq!(
            v,
            Yaml::Seq(vec![
                Yaml::Str("a".into()),
                Yaml::Seq(vec![Yaml::Int(1), Yaml::Int(2)])
            ])
        );
    }
}
