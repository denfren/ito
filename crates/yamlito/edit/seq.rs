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
//!   spaced. A *regular* one-item-per-line multiline layout
//!   (`[\n  a,\n  b,\n]`) is also continued — new items keep the detected
//!   per-item indent, and the trailing-comma-or-not style is preserved.
//!   Irregular shapes (the first item sharing the `[` line, mismatched
//!   indents, a stray newline mid-list) fall back to inline spacing
//!   rather than risk mangling them; the result still reparses.
//!
//! A block sequence can never be empty (an empty list is only spelled
//! `[]` in flow), so removing the *last* block item converts the whole
//! `BLOCK_SEQUENCE` into a flow `[]`, keeping the tree valid and
//! round-trippable. Conversely, pushing into that `[]` grows the flow
//! sequence.
//!
//! Item fragments are produced through the value-model builder: the
//! value is serialized to canonical YAML, parsed, and the resulting
//! green subtree spliced — so an inserted item is always parse- and
//! round-trip-consistent.

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
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

/// Insert `value` into the sequence `seq` at item position `index`
/// (clamped to the item count). Dispatches on the node kind: a
/// `BLOCK_SEQUENCE` gets a dash item on its own line; a `FLOW_SEQUENCE`
/// gets a `[...]` item with separator spacing matching its neighbours.
///
/// `seq` must be a `BLOCK_SEQUENCE` or `FLOW_SEQUENCE` belonging to
/// `tree`. Returns the *live* sequence node after the edit — rowan's
/// `ReplaceNode` detaches the old node, so the caller should hold this
/// returned handle instead of their old `seq`.
pub fn insert_seq_item(
    tree: &mut SyntaxTree,
    seq: &SyntaxNode,
    index: usize,
    value: Yaml,
) -> SyntaxNode {
    match seq.kind() {
        SyntaxKind::BLOCK_SEQUENCE => insert_block_item(tree, seq, index, value),
        SyntaxKind::FLOW_SEQUENCE => insert_flow_item(tree, seq, index, value),
        other => {
            debug_assert!(false, "insert_seq_item: not a sequence: {other:?}");
            seq.clone()
        }
    }
}

/// Append `value` after the last item of `seq`. Convenience for
/// `insert_seq_item(tree, seq, usize::MAX, value)`. Returns the live
/// sequence node after the edit (see [`insert_seq_item`]).
pub fn push_seq_item(tree: &mut SyntaxTree, seq: &SyntaxNode, value: Yaml) -> SyntaxNode {
    insert_seq_item(tree, seq, usize::MAX, value)
}

/// Remove the item at position `index` from `seq`, together with the
/// adjacent separator (NEWLINE for block, `,`/WHITESPACE for flow) that
/// joins it to the rest, so no stray separator is left behind. No-op
/// when out of range.
///
/// Removing the last remaining item of a *block* sequence converts the
/// whole node into an empty flow `[]` (a block sequence cannot be
/// empty); removing the last item of a flow sequence leaves `[]`.
///
/// Returns the *live* sequence node after the edit — the new node a
/// caller should hold instead of their old `seq`. When the last block
/// item is removed this is the freshly inserted flow `[]` node; for an
/// out-of-range no-op it is `seq` unchanged.
pub fn remove_seq_item(tree: &mut SyntaxTree, seq: &SyntaxNode, index: usize) -> SyntaxNode {
    match seq.kind() {
        SyntaxKind::BLOCK_SEQUENCE => remove_block_item(tree, seq, index),
        SyntaxKind::FLOW_SEQUENCE => remove_flow_item(tree, seq, index),
        other => {
            debug_assert!(false, "remove_seq_item: not a sequence: {other:?}");
            seq.clone()
        }
    }
}

// ---------------------------------------------------------------------------
// Block sequences
// ---------------------------------------------------------------------------

fn insert_block_item(
    tree: &mut SyntaxTree,
    seq: &SyntaxNode,
    index: usize,
    value: Yaml,
) -> SyntaxNode {
    let col = collection_col(seq, SyntaxKind::BLOCK_SEQUENCE_ENTRY);
    let item_green = build_block_item_green(value, col);
    block_insert(
        tree,
        seq,
        index,
        SyntaxKind::BLOCK_SEQUENCE_ENTRY,
        col,
        item_green,
    )
}

fn remove_block_item(tree: &mut SyntaxTree, seq: &SyntaxNode, index: usize) -> SyntaxNode {
    block_remove(
        tree,
        seq,
        index,
        SyntaxKind::BLOCK_SEQUENCE_ENTRY,
        // A block sequence cannot be empty, so the last item collapses the
        // whole node to a flow `[]`.
        |tree, seq| {
            collapse_block_collection_to_empty(
                tree,
                seq,
                SyntaxKind::FLOW_SEQUENCE,
                (SyntaxKind::L_BRACKET, "["),
                (SyntaxKind::R_BRACKET, "]"),
            )
        },
    )
}

/// Build the green `BLOCK_SEQUENCE_ENTRY` for `- value`, indented for a
/// sequence at column `col`. We serialize a single-item sequence, parse
/// its CANONICAL column-0 text (which the builder guarantees parses),
/// lift its entry subtree, then push `col` extra spaces onto every
/// interior continuation line via [`indent_green_continuations`] so the
/// multi-line item aligns under a sequence at `col`.
///
/// The entry's own first line stays bare: the caller supplies its leading
/// indent as a sibling WHITESPACE token (see [`insert_block_item`]).
fn build_block_item_green(value: Yaml, col: usize) -> rowan::GreenNode {
    let fragment = Yaml::Seq(vec![value]);
    let text = fragment.to_yaml_string();
    let tree = crate::parse(&text).expect("builder fragment must parse");
    let entry = tree
        .root()
        .descendants()
        .find(|n| n.kind() == SyntaxKind::BLOCK_SEQUENCE_ENTRY)
        .expect("fragment has a sequence entry");
    indent_green_continuations(&entry, col)
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
/// Strings are quoted with the strict **flow** policy
/// ([`quote_scalar_flow`]) so flow terminators (`,` `[` `]` `{` `}`) and
/// the `: ` mapping indicator can never leak through and corrupt the
/// surrounding collection. Other scalars use the builder's canonical
/// form. Collections recurse, quoting keys *and* values the same way.
fn flow_scalar_text(value: &Yaml) -> String {
    match value {
        // Flow-quoted directly: `to_yaml_string` applies only block rules,
        // which do not protect flow terminators.
        Yaml::Str(s) => quote_scalar_flow(s),
        Yaml::Null | Yaml::Bool(_) | Yaml::Int(_) | Yaml::Float(_) => {
            value.to_yaml_string().trim_end_matches('\n').to_string()
        }
        Yaml::Seq(items) => {
            let inner: Vec<String> = items.iter().map(flow_scalar_text).collect();
            format!("[{}]", inner.join(", "))
        }
        Yaml::Map(pairs) => {
            let inner: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", quote_scalar_flow(k), flow_scalar_text(v)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

/// The separator placed between flow-sequence items, sampled from the
/// existing layout so new items blend in.
enum FlowSep {
    /// Inline, single-line: a comma optionally followed by whitespace
    /// (`[a, b]` → `", "`; `[a,b]` → `""`).
    Inline(String),
    /// One-item-per-line block layout: a comma, then a newline, then the
    /// per-item indent (`[\n  a,\n  b,\n]` → indent `"  "`).
    Multiline(String),
}

impl FlowSep {
    /// Emit the separator tokens that follow a comma, before the next
    /// item. The caller supplies the comma itself.
    fn after_comma(&self) -> Vec<GreenElement> {
        match self {
            FlowSep::Inline(ws) if ws.is_empty() => Vec::new(),
            FlowSep::Inline(ws) => vec![NodeOrToken::Token(mk_token(SyntaxKind::WHITESPACE, ws))],
            FlowSep::Multiline(indent) => {
                let mut out = vec![NodeOrToken::Token(mk_token(SyntaxKind::NEWLINE, "\n"))];
                if !indent.is_empty() {
                    out.push(NodeOrToken::Token(mk_token(SyntaxKind::WHITESPACE, indent)));
                }
                out
            }
        }
    }
}

/// Sample the comma-to-item separator used inside `seq`.
///
/// - `[a, b]` / `[a,b]` → [`FlowSep::Inline`] reusing the existing
///   inter-item whitespace.
/// - `[\n  a,\n  b,\n]` and friends → [`FlowSep::Multiline`] reusing the
///   detected per-item indent, but only when the layout is *regular*
///   (every item begins at the start of a line at the same column). Any
///   irregular multiline shape (e.g. the first item sharing the bracket
///   line, mismatched indents) falls back to inline so we don't mangle
///   it.
/// - single-item / empty list → default `FlowSep::Inline(" ")`.
fn flow_separator(seq: &SyntaxNode) -> FlowSep {
    let children: Vec<_> = seq.children_with_tokens().collect();

    // Find the first comma and what immediately follows it.
    let comma_pos = children
        .iter()
        .position(|el| matches!(el, NodeOrToken::Token(t) if t.kind() == SyntaxKind::COMMA));

    if let Some(i) = comma_pos {
        match children.get(i + 1) {
            Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::WHITESPACE => {
                return FlowSep::Inline(t.text().to_string());
            }
            Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::NEWLINE => {
                // Looks multiline. Only honour it when the layout is
                // regular enough to reproduce safely.
                if let Some(indent) = regular_multiline_indent(&children) {
                    return FlowSep::Multiline(indent);
                }
                return FlowSep::Inline(String::new());
            }
            // Comma directly followed by the next item: tight inline.
            _ => return FlowSep::Inline(String::new()),
        }
    }

    // No comma to sample (single-item or empty list). A lone multiline
    // item (`[\n  a,\n]`) still gives us a regular indent to continue.
    if let Some(indent) = regular_multiline_indent(&children) {
        return FlowSep::Multiline(indent);
    }
    FlowSep::Inline(" ".to_string())
}

/// If `seq`'s children describe a regular one-item-per-line layout —
/// every `FLOW_SEQUENCE_ENTRY` is immediately preceded by a NEWLINE then
/// a WHITESPACE of one shared width (or directly by a NEWLINE at column
/// 0) — return that indent. Otherwise `None`.
///
/// "Regular" deliberately excludes the case where the first item shares
/// the `[` line (`[a,\n b]`): that item has no leading NEWLINE, so it
/// fails the check and we leave the weird layout alone.
fn regular_multiline_indent(children: &[SyntaxElement]) -> Option<String> {
    let mut indent: Option<String> = None;
    let mut saw_item = false;
    for (i, el) in children.iter().enumerate() {
        let NodeOrToken::Node(n) = el else { continue };
        if n.kind() != SyntaxKind::FLOW_SEQUENCE_ENTRY {
            continue;
        }
        saw_item = true;
        // Determine the indent immediately before this item.
        let this = match (
            children.get(i.wrapping_sub(1)),
            children.get(i.wrapping_sub(2)),
        ) {
            // NEWLINE WHITESPACE ITEM
            (Some(NodeOrToken::Token(ws)), Some(NodeOrToken::Token(nl)))
                if i >= 2
                    && ws.kind() == SyntaxKind::WHITESPACE
                    && nl.kind() == SyntaxKind::NEWLINE =>
            {
                ws.text().to_string()
            }
            // NEWLINE ITEM (column 0)
            (Some(NodeOrToken::Token(nl)), _) if nl.kind() == SyntaxKind::NEWLINE => String::new(),
            // Anything else (e.g. item right after `[`): irregular.
            _ => return None,
        };
        match &indent {
            None => indent = Some(this),
            Some(prev) if *prev == this => {}
            // Mismatched indents across items: irregular.
            Some(_) => return None,
        }
    }
    if saw_item { indent } else { None }
}

/// True when a flow entry node's last token is a NEWLINE — the shape the
/// parser produces for the final item of a no-trailing-comma multiline
/// list (`b\n` immediately before `]`).
fn entry_ends_with_newline(entry: &SyntaxNode) -> bool {
    matches!(entry.last_token(), Some(t) if t.kind() == SyntaxKind::NEWLINE)
}

/// Rebuild `entry`'s green with its trailing NEWLINE token stripped.
/// Caller has already confirmed the entry ends with one (see
/// [`entry_ends_with_newline`]).
fn entry_without_trailing_newline(entry: &SyntaxNode) -> rowan::GreenNode {
    let newline_raw =
        <crate::syntax::YamlLang as rowan::Language>::kind_to_raw(SyntaxKind::NEWLINE);
    let mut kept: Vec<GreenElement> = entry
        .green()
        .children()
        .map(|c| match c {
            rowan::NodeOrToken::Node(n) => NodeOrToken::Node(n.to_owned()),
            rowan::NodeOrToken::Token(t) => NodeOrToken::Token(t.to_owned()),
        })
        .collect();
    if matches!(kept.last(), Some(NodeOrToken::Token(t)) if t.kind() == newline_raw) {
        kept.pop();
    }
    mk_node(SyntaxKind::FLOW_SEQUENCE_ENTRY, kept)
}

fn insert_flow_item(
    tree: &mut SyntaxTree,
    seq: &SyntaxNode,
    index: usize,
    value: Yaml,
) -> SyntaxNode {
    let item_green = build_flow_item_green(value);
    let item_indices = entry_child_indices(seq, SyntaxKind::FLOW_SEQUENCE_ENTRY);
    let sep = flow_separator(seq);

    // Locate the closing bracket so an item dropped into an empty `[]`
    // lands just inside it.
    let children: Vec<_> = seq.children_with_tokens().collect();
    let r_bracket_idx = children
        .iter()
        .position(|el| matches!(el, NodeOrToken::Token(t) if t.kind() == SyntaxKind::R_BRACKET));

    let comma = || NodeOrToken::Token(mk_token(SyntaxKind::COMMA, ","));

    if item_indices.is_empty() {
        // Empty `[]` — drop the item just before `]` with no separator.
        let at = r_bracket_idx.unwrap_or(children.len());
        let frag = vec![NodeOrToken::Node(item_green)];
        let new_green = splice_children_green(seq, at..at, frag);
        return replace_node_live(tree, seq, new_green);
    }

    let (range, fragment): (std::ops::Range<usize>, Vec<GreenElement>) =
        if index >= item_indices.len() {
            // Append after the last item: `,<sep>item` (matching observed
            // layout). For multiline, this becomes `,\n<indent>item`, and the
            // item lands before the closing-bracket line.
            let last = *item_indices.last().unwrap();

            // A no-trailing-comma multiline layout (`[\n  a,\n  b\n]`) keeps
            // the line-ending NEWLINE *inside* the last entry (`b\n`). Naively
            // splicing `,...` after the entry would land it past that newline
            // (`b\n,...`). Detect this and rebuild the tail so the newline
            // stays the closer: `b` + `,\n  c` + `\n`. This replaces the last
            // entry rather than inserting after it.
            if let (FlowSep::Multiline(indent), Some(NodeOrToken::Node(entry))) =
                (&sep, children.get(last))
                && entry_ends_with_newline(entry)
            {
                let clean = entry_without_trailing_newline(entry);
                let mut frag = vec![NodeOrToken::Node(clean), comma()];
                frag.extend(FlowSep::Multiline(indent.clone()).after_comma());
                frag.push(NodeOrToken::Node(item_green));
                frag.push(NodeOrToken::Token(mk_token(SyntaxKind::NEWLINE, "\n")));
                (last..last + 1, frag)
            } else {
                let at = last + 1;
                let mut frag = vec![comma()];
                frag.extend(sep.after_comma());
                frag.push(NodeOrToken::Node(item_green));
                (at..at, frag)
            }
        } else {
            // Insert before the item at `index`: `item,<sep>` so the existing
            // item keeps its own leading indent/position.
            let at = item_indices[index];
            let mut frag = vec![NodeOrToken::Node(item_green), comma()];
            frag.extend(sep.after_comma());
            (at..at, frag)
        };

    let new_green = splice_children_green(seq, range, fragment);
    replace_node_live(tree, seq, new_green)
}

fn remove_flow_item(tree: &mut SyntaxTree, seq: &SyntaxNode, index: usize) -> SyntaxNode {
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
        return seq.clone();
    };

    // Removing the only item: collapse to a clean `[]`, dropping any
    // interior trivia (a stray comma/newline would otherwise linger).
    if item_positions.len() == 1 {
        let empty = mk_node(
            SyntaxKind::FLOW_SEQUENCE,
            [
                NodeOrToken::Token(mk_token(SyntaxKind::L_BRACKET, "[")),
                NodeOrToken::Token(mk_token(SyntaxKind::R_BRACKET, "]")),
            ],
        );
        return replace_node_live(tree, seq, empty);
    }

    let is_comma = |i: usize| matches!(children.get(i), Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::COMMA);
    let is_ws = |i: usize| matches!(children.get(i), Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::WHITESPACE);
    let is_newline = |i: usize| matches!(children.get(i), Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::NEWLINE);
    // A no-trailing-comma multiline last item keeps the closing NEWLINE
    // inside its own node (`b\n` immediately before `]`).
    let entry_owns_closing_newline = matches!(
        children.get(item_idx),
        Some(NodeOrToken::Node(n)) if entry_ends_with_newline(n)
    );

    // Multiline when a NEWLINE sits between the brackets — removal then has
    // to drop a whole line (the item's own indent + its trailing
    // comma/newline) so no blank indented line is left behind.
    let multiline = children
        .iter()
        .any(|el| matches!(el, NodeOrToken::Token(t) if t.kind() == SyntaxKind::NEWLINE));

    let mut start = item_idx;
    let mut end = item_idx + 1;
    // Replacement spliced in place of the removed range; normally empty,
    // but the multiline last item may need a NEWLINE kept so `]` stays on
    // its own line.
    let mut replacement: Vec<GreenElement> = Vec::new();

    if index + 1 < item_positions.len() {
        // Not the last item: the successor should slide into this slot.
        if multiline {
            // Drop the item's own leading indent and its trailing
            // `,`+NEWLINE, leaving the next item's indent in place.
            while start > 0 && is_ws(start - 1) {
                start -= 1;
            }
            if is_comma(end) {
                end += 1;
            }
            if is_newline(end) {
                end += 1;
            }
        } else {
            // Inline: swallow the following `,` and any WHITESPACE up to the
            // next item, so the successor keeps its leading spacing.
            if is_comma(end) {
                end += 1;
            }
            while is_ws(end) {
                end += 1;
            }
        }
    } else {
        // Last (but not only) item: swallow the *preceding* `,` and the
        // whitespace/newline around it, so no dangling separator remains.
        if is_ws(start.saturating_sub(1)) && start > 0 {
            start -= 1;
        }
        if start > 0 && is_newline(start - 1) {
            start -= 1;
        }
        if start > 0 && is_comma(start - 1) {
            start -= 1;
        }
        // Also drop whitespace before that comma (rare, e.g. `[a , b]`).
        while start > 0 && is_ws(start - 1) {
            start -= 1;
        }
        // If this item owned the only closing NEWLINE (no-trailing-comma
        // multiline), removing it would pull `]` up onto the previous
        // item's line. Keep one NEWLINE so the bracket stays on its own
        // line, matching the surviving layout.
        if multiline && entry_owns_closing_newline {
            replacement.push(NodeOrToken::Token(mk_token(SyntaxKind::NEWLINE, "\n")));
        }
    }

    let new_green = splice_children_green(seq, start..end, replacement);
    replace_node_live(tree, seq, new_green)
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

    /// The returned node is the live block sequence and can drive a second
    /// edit directly, without re-finding it in the tree.
    #[test]
    fn block_push_returns_live_node_for_chained_edit() {
        let mut tree = crate::parse("- a\n- b\n").unwrap();
        let seq = block_seq(&tree);
        let new = push_seq_item(&mut tree, &seq, Yaml::Str("c".into()));
        assert_eq!(new.kind(), SyntaxKind::BLOCK_SEQUENCE);
        // Issue a second edit through the returned handle.
        let n2 = push_seq_item(&mut tree, &new, Yaml::Str("d".into()));
        assert_eq!(n2.kind(), SyntaxKind::BLOCK_SEQUENCE);
        let out = tree.emit();
        assert_eq!(out, "- a\n- b\n- c\n- d\n");
        assert!(out.contains("- c\n"));
        assert!(out.contains("- d\n"));
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
        push_seq_item(&mut tree, &seq, Yaml::Map(vec![("k".into(), Yaml::Int(2))]));
        assert_eq!(tree.emit(), "- k: 1\n- k: 2\n");
        reparse(&tree);
    }

    // ---- block: multi-line item values (maps, nested seqs) ----
    //
    // Regression tests for the panic in `build_block_item_green`: pushing
    // a MAP (or nested seq) — any multi-line value — into a BLOCK sequence
    // used to reparse standalone with `col` baked into the continuation
    // lines, mangling the indentation. The fix reindents the parsed
    // entry's continuation lines at the green level instead.

    #[test]
    fn block_push_multiline_map_top_level() {
        let mut tree = crate::parse("- a\n").unwrap();
        let seq = block_seq(&tree);
        push_seq_item(
            &mut tree,
            &seq,
            Yaml::Map(vec![("k".into(), Yaml::Int(1)), ("j".into(), Yaml::Int(2))]),
        );
        assert_eq!(tree.emit(), "- a\n- k: 1\n  j: 2\n");
        reparse(&tree);
    }

    #[test]
    fn block_push_multiline_map_nested_col2() {
        // The original panic repro: a map pushed into a 2-space sequence.
        let mut tree = crate::parse("items:\n  - a\n").unwrap();
        let seq = block_seq(&tree);
        push_seq_item(
            &mut tree,
            &seq,
            Yaml::Map(vec![("k".into(), Yaml::Int(1)), ("j".into(), Yaml::Int(2))]),
        );
        assert_eq!(tree.emit(), "items:\n  - a\n  - k: 1\n    j: 2\n");
        reparse(&tree);
    }

    #[test]
    fn block_push_multiline_map_deep_col4() {
        let mut tree = crate::parse("a:\n  b:\n    - x\n").unwrap();
        let seq = block_seq(&tree);
        push_seq_item(
            &mut tree,
            &seq,
            Yaml::Map(vec![("k".into(), Yaml::Int(1)), ("j".into(), Yaml::Int(2))]),
        );
        assert_eq!(tree.emit(), "a:\n  b:\n    - x\n    - k: 1\n      j: 2\n");
        reparse(&tree);
    }

    #[test]
    fn block_insert_front_multiline_map() {
        let mut tree = crate::parse("items:\n  - b\n").unwrap();
        let seq = block_seq(&tree);
        insert_seq_item(
            &mut tree,
            &seq,
            0,
            Yaml::Map(vec![("k".into(), Yaml::Int(1)), ("j".into(), Yaml::Int(2))]),
        );
        assert_eq!(tree.emit(), "items:\n  - k: 1\n    j: 2\n  - b\n");
        reparse(&tree);
    }

    #[test]
    fn block_insert_middle_multiline_map() {
        let mut tree = crate::parse("items:\n  - a\n  - c\n").unwrap();
        let seq = block_seq(&tree);
        insert_seq_item(
            &mut tree,
            &seq,
            1,
            Yaml::Map(vec![("k".into(), Yaml::Int(1)), ("j".into(), Yaml::Int(2))]),
        );
        assert_eq!(tree.emit(), "items:\n  - a\n  - k: 1\n    j: 2\n  - c\n");
        reparse(&tree);
    }

    #[test]
    fn block_push_nested_seq_value() {
        // A nested sequence is also multi-line: canonical form is
        // `-\n  - 1\n  - 2\n  - 3`.
        let mut tree = crate::parse("items:\n  - a\n").unwrap();
        let seq = block_seq(&tree);
        push_seq_item(
            &mut tree,
            &seq,
            Yaml::Seq(vec![Yaml::Int(1), Yaml::Int(2), Yaml::Int(3)]),
        );
        assert_eq!(
            tree.emit(),
            "items:\n  - a\n  -\n    - 1\n    - 2\n    - 3\n"
        );
        reparse(&tree);
        let v = Yaml::from_tree(&tree).unwrap();
        assert_eq!(
            v,
            Yaml::Map(vec![(
                "items".into(),
                Yaml::Seq(vec![
                    Yaml::Str("a".into()),
                    Yaml::Seq(vec![Yaml::Int(1), Yaml::Int(2), Yaml::Int(3)]),
                ]),
            )])
        );
    }

    // The value-model builder never emits literal (`|`) / folded (`>`)
    // block scalars — a `Yaml::Str` containing a newline is double-quoted
    // with `\n` escapes (see `quoting::quote_scalar_block`), which is a
    // regular flow scalar with no significant interior indentation. So the
    // block-scalar caveat of `indent_green_continuations` cannot be hit
    // through THIS code path; the helper still handles it safely for
    // reuse, and `tree_utils` carries a direct test for that case.

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

    /// The collapse-to-`[]` path returns the live FLOW_SEQUENCE node, so a
    /// chained push can grow it without re-finding it via `flow_seq`.
    #[test]
    fn block_remove_only_item_returns_live_flow_node() {
        let mut tree = crate::parse("items:\n  - a\n").unwrap();
        let seq = block_seq(&tree);
        let empty = remove_seq_item(&mut tree, &seq, 0);
        assert_eq!(tree.emit(), "items: []\n");
        assert_eq!(empty.kind(), SyntaxKind::FLOW_SEQUENCE);
        // Grow the returned `[]` node directly.
        let grown = push_seq_item(&mut tree, &empty, Yaml::Str("b".into()));
        assert_eq!(grown.kind(), SyntaxKind::FLOW_SEQUENCE);
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

    /// The returned node is the live flow sequence and supports a chained
    /// second edit.
    #[test]
    fn flow_push_returns_live_node_for_chained_edit() {
        let mut tree = crate::parse("[a, b]\n").unwrap();
        let seq = flow_seq(&tree);
        let new = push_seq_item(&mut tree, &seq, Yaml::Str("c".into()));
        assert_eq!(new.kind(), SyntaxKind::FLOW_SEQUENCE);
        let n2 = push_seq_item(&mut tree, &new, Yaml::Str("d".into()));
        assert_eq!(n2.kind(), SyntaxKind::FLOW_SEQUENCE);
        let out = tree.emit();
        assert_eq!(out, "[a, b, c, d]\n");
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

    // ---- flow: multiline layout continuation ----

    #[test]
    fn flow_push_multiline_trailing_comma() {
        let mut tree = crate::parse("[\n  a,\n  b,\n]\n").unwrap();
        let seq = flow_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("c".into()));
        assert_eq!(tree.emit(), "[\n  a,\n  b,\n  c,\n]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_push_multiline_no_trailing_comma() {
        // Last item owns its closing NEWLINE (`b\n`); the appended item
        // keeps the no-trailing-comma style with `]` on its own line.
        let mut tree = crate::parse("[\n  a,\n  b\n]\n").unwrap();
        let seq = flow_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("c".into()));
        assert_eq!(tree.emit(), "[\n  a,\n  b,\n  c\n]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_push_multiline_single_item() {
        let mut tree = crate::parse("[\n  a,\n]\n").unwrap();
        let seq = flow_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("b".into()));
        assert_eq!(tree.emit(), "[\n  a,\n  b,\n]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_push_multiline_four_space_indent() {
        let mut tree = crate::parse("[\n    a,\n    b,\n]\n").unwrap();
        let seq = flow_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("c".into()));
        assert_eq!(tree.emit(), "[\n    a,\n    b,\n    c,\n]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_push_multiline_under_key_preserves_indent() {
        let mut tree = crate::parse("items:\n  [\n    a,\n    b,\n  ]\n").unwrap();
        let seq = flow_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("c".into()));
        assert_eq!(tree.emit(), "items:\n  [\n    a,\n    b,\n    c,\n  ]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_insert_front_multiline() {
        let mut tree = crate::parse("[\n  a,\n  b,\n]\n").unwrap();
        let seq = flow_seq(&tree);
        insert_seq_item(&mut tree, &seq, 0, Yaml::Str("z".into()));
        assert_eq!(tree.emit(), "[\n  z,\n  a,\n  b,\n]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_insert_middle_multiline() {
        let mut tree = crate::parse("[\n  a,\n  c,\n]\n").unwrap();
        let seq = flow_seq(&tree);
        insert_seq_item(&mut tree, &seq, 1, Yaml::Str("b".into()));
        assert_eq!(tree.emit(), "[\n  a,\n  b,\n  c,\n]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_insert_before_last_multiline_no_trailing_comma() {
        let mut tree = crate::parse("[\n  a,\n  b\n]\n").unwrap();
        let seq = flow_seq(&tree);
        insert_seq_item(&mut tree, &seq, 1, Yaml::Str("x".into()));
        assert_eq!(tree.emit(), "[\n  a,\n  x,\n  b\n]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_remove_multiline_first() {
        let mut tree = crate::parse("[\n  a,\n  b,\n  c,\n]\n").unwrap();
        let seq = flow_seq(&tree);
        remove_seq_item(&mut tree, &seq, 0);
        assert_eq!(tree.emit(), "[\n  b,\n  c,\n]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_remove_multiline_middle() {
        let mut tree = crate::parse("[\n  a,\n  b,\n  c,\n]\n").unwrap();
        let seq = flow_seq(&tree);
        remove_seq_item(&mut tree, &seq, 1);
        assert_eq!(tree.emit(), "[\n  a,\n  c,\n]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_remove_multiline_last_trailing_comma() {
        let mut tree = crate::parse("[\n  a,\n  b,\n]\n").unwrap();
        let seq = flow_seq(&tree);
        remove_seq_item(&mut tree, &seq, 1);
        assert_eq!(tree.emit(), "[\n  a,\n]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_remove_multiline_last_no_trailing_comma() {
        // Removing the newline-owning last item keeps `]` on its own line.
        let mut tree = crate::parse("[\n  a,\n  b\n]\n").unwrap();
        let seq = flow_seq(&tree);
        remove_seq_item(&mut tree, &seq, 1);
        assert_eq!(tree.emit(), "[\n  a\n]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_remove_multiline_only_item_collapses() {
        let mut tree = crate::parse("[\n  a,\n]\n").unwrap();
        let seq = flow_seq(&tree);
        remove_seq_item(&mut tree, &seq, 0);
        assert_eq!(tree.emit(), "[]\n");
        reparse(&tree);
    }

    // ---- flow: stray newline in an otherwise inline list ----
    //
    // An irregular layout (the first item shares the `[` line, or only
    // some items are newline-led) fails the regularity gate, so new items
    // fall back to inline spacing. The stray newline is left where it is
    // and every result still reparses.

    #[test]
    fn flow_push_stray_newline_after_first_comma_falls_back_inline() {
        let mut tree = crate::parse("[a,\nb, c]\n").unwrap();
        let seq = flow_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("d".into()));
        assert_eq!(tree.emit(), "[a,\nb, c,d]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_push_leading_newline_only_falls_back_inline() {
        let mut tree = crate::parse("[\na, b]\n").unwrap();
        let seq = flow_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Str("c".into()));
        assert_eq!(tree.emit(), "[\na, b, c]\n");
        reparse(&tree);
    }

    #[test]
    fn flow_remove_stray_newline_reparses() {
        let mut tree = crate::parse("[a,\nb, c]\n").unwrap();
        let seq = flow_seq(&tree);
        remove_seq_item(&mut tree, &seq, 0);
        reparse(&tree);
        let v = Yaml::from_tree(&tree).unwrap();
        assert_eq!(
            v,
            Yaml::Seq(vec![Yaml::Str("b".into()), Yaml::Str("c".into())])
        );
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

    // ---- flow: hostile scalar values quote correctly ----
    //
    // A string containing a flow terminator (`,` `]` `}`) or the `: `
    // mapping indicator must be quoted when spliced into a flow sequence,
    // or it silently corrupts the surrounding collection (data loss) or
    // panics on reparse.

    /// Push a `Yaml` value into a `[x]` flow seq and assert it reparses
    /// and decodes back to `[x_value, value]` exactly.
    fn assert_flow_push_round_trips(value: Yaml) {
        let mut tree = crate::parse("[x]\n").unwrap();
        let seq = flow_seq(&tree);
        push_seq_item(&mut tree, &seq, value.clone());
        reparse(&tree);
        let v = Yaml::from_tree(&tree).unwrap();
        assert_eq!(v, Yaml::Seq(vec![Yaml::Str("x".into()), value]));
    }

    #[test]
    fn flow_push_string_with_comma_no_data_loss() {
        assert_flow_push_round_trips(Yaml::Str("a,b".into()));
    }

    #[test]
    fn flow_push_string_with_close_bracket_no_panic() {
        assert_flow_push_round_trips(Yaml::Str("a]b".into()));
    }

    #[test]
    fn flow_push_string_with_close_brace() {
        assert_flow_push_round_trips(Yaml::Str("a}b".into()));
    }

    #[test]
    fn flow_push_string_with_colon_space() {
        assert_flow_push_round_trips(Yaml::Str("a: b".into()));
    }

    #[test]
    fn flow_push_map_with_hostile_key_round_trips() {
        // Flow-map key `"a: b"` must be quoted: unquoted it reparses as
        // key `a`, value `b: 1`.
        assert_flow_push_round_trips(Yaml::Map(vec![("a: b".into(), Yaml::Int(1))]));
    }

    #[test]
    fn flow_push_map_with_comma_key_and_value() {
        assert_flow_push_round_trips(Yaml::Map(vec![("k,1".into(), Yaml::Str("v,2".into()))]));
    }

    #[test]
    fn flow_inserted_nested_seq_reads_back() {
        let mut tree = crate::parse("[a]\n").unwrap();
        let seq = flow_seq(&tree);
        push_seq_item(&mut tree, &seq, Yaml::Seq(vec![Yaml::Int(1), Yaml::Int(2)]));
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
