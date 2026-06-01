//! Shared machinery for reading and writing comment blocks on AST
//! nodes that admit them: `BlockMappingEntry`, `BlockSequenceEntry`,
//! and `Document`.
//!
//! Each of those types has its own quirks — first-nested-element
//! walk-up for sequences, stream-vs-document trivia placement for
//! documents — so the entry points live on the types themselves
//! (see `src/ast/nodes.rs`). This module hosts the helpers they all
//! share: payload normalization on read, emission rules on write,
//! and `splice_children_green`-based replacement.
//!
//! ## Normalization (read)
//!
//! Each `COMMENT` token's leading `#` is dropped. If the next byte is
//! a single ` `, it's also dropped (the conventional separator, not
//! payload). Lines are joined with `\n`. A blank-line split in the
//! trivia splits the block; only the run immediately before the
//! anchor is returned.
//!
//! ## Emission (write)
//!
//! Each `\n`-separated line of the input is written as `# <line>`
//! (single space between `#` and content, `#` alone for empty lines).
//! Indentation is reproduced from the anchor's column.

use rowan::NodeOrToken;

use crate::SyntaxTree;
use crate::edit::{GreenElement, TreeEdit, apply_edit, mk_token, splice_children_green};
use crate::syntax::tree_utils::{indent_of, is_first_meaningful_child};
use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

/// Read the leading comment block attached to `anchor` — the contiguous
/// run of `COMMENT` tokens (possibly separated by `NEWLINE`/`WHITESPACE`
/// trivia) immediately preceding `anchor` within its parent. Empty
/// string when absent or when the anchor has no parent.
///
/// Walks up through ancestors when `anchor` is the first meaningful
/// child at its level — this promotes STREAM-level doc-leading trivia
/// to the first entry inside the document.
///
/// Callers: `BlockMappingEntry::leading_comment`, similar.
pub(crate) fn read_leading(anchor: &SyntaxNode) -> String {
    let Some(block) = find_leading_block_with_walk_up(anchor) else {
        return String::new();
    };
    render_payload(&block.comments)
}

fn find_leading_block_with_walk_up(anchor: &SyntaxNode) -> Option<LeadingBlock> {
    let mut cur = anchor.clone();
    loop {
        if let Some(block) = find_leading_block(&cur) {
            return Some(block);
        }
        if !is_first_meaningful_child(&cur) {
            return None;
        }
        cur = cur.parent()?;
    }
}

/// Replace the leading comment block on `anchor` with `text` (or clear
/// when `None`). When no existing block is attached to `anchor` but a
/// block lives further up (e.g. STREAM-level trivia that `read_leading`
/// attributes to this entry), replace it in place so read and write
/// round-trip consistently.
pub(crate) fn write_leading(tree: &mut SyntaxTree, anchor: &SyntaxNode, text: Option<&str>) {
    if let Some(existing) = find_leading_block_with_walk_up(anchor) {
        let replacement =
            build_leading_elements(text, &existing.indent, LeadingMode::ReplaceFromLineStart);
        let new_green = splice_children_green(&existing.parent, existing.range, replacement);
        apply_edit(
            tree,
            TreeEdit::ReplaceNode {
                target: existing.parent,
                new: new_green,
            },
        );
        return;
    }
    let Some(text) = text else {
        return; // nothing to clear
    };
    let Some(parent) = anchor.parent() else {
        return;
    };

    // Flow entries keep their leading indent as a preceding sibling at
    // the collection level (not inside the entry). Insert the comment
    // line(s) by replacing that indent run, re-establishing the entry's
    // column afterward — the same shape as ReplaceFromLineStart.
    if in_flow(anchor) {
        let indent = indent_of(anchor);
        // Range: any WHITESPACE immediately before the anchor, up to the
        // anchor itself.
        let mut start = anchor.index();
        while start > 0 {
            match parent.children_with_tokens().nth(start - 1) {
                Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::WHITESPACE => start -= 1,
                _ => break,
            }
        }
        let anchor_index = anchor.index();
        let replacement =
            build_leading_elements(Some(text), &indent, LeadingMode::ReplaceFromLineStart);
        let new_green = splice_children_green(&parent, start..anchor_index, replacement);
        apply_edit(
            tree,
            TreeEdit::ReplaceNode {
                target: parent,
                new: new_green,
            },
        );
        return;
    }

    let anchor_index = anchor.index();
    let indent = indent_of(anchor);
    // Uniform entry-indent ownership (Phase 2): a block entry either owns
    // its leading indent WHITESPACE as its first child (line-start entry)
    // or has no indent at all (col-0 / mid-line entry, where `indent_of`
    // returns ""). In both cases we emit the indent before every comment
    // line and omit a trailing indent — when the anchor owns the indent
    // it re-establishes its own column, and when it doesn't the indent is
    // empty so there's nothing to duplicate.
    let replacement = build_leading_elements(Some(text), &indent, LeadingMode::InsertBeforeAnchor);
    let new_green = splice_children_green(&parent, anchor_index..anchor_index, replacement);
    apply_edit(
        tree,
        TreeEdit::ReplaceNode {
            target: parent,
            new: new_green,
        },
    );
}

/// Location of an existing leading comment block within some parent.
pub(crate) struct LeadingBlock {
    comments: Vec<SyntaxToken>,
    parent: SyntaxNode,
    range: std::ops::Range<usize>,
    indent: String,
}

fn find_leading_block(anchor: &SyntaxNode) -> Option<LeadingBlock> {
    let parent = anchor.parent()?;
    let anchor_index = anchor.index();
    let children: Vec<_> = parent.children_with_tokens().collect();

    // Walk back collecting contiguous trivia.
    let mut start = anchor_index;
    while start > 0 {
        let prev = &children[start - 1];
        match prev {
            NodeOrToken::Token(t) if t.kind().is_trivia() => start -= 1,
            _ => break,
        }
    }

    // Filter to the contiguous block ending at the anchor. Blank lines
    // split the block — only the trailing run counts.
    let trivia_slice = &children[start..anchor_index];
    let comments = contiguous_trailing_comments(trivia_slice);
    if comments.is_empty() {
        return None;
    }

    Some(LeadingBlock {
        comments,
        parent,
        range: start..anchor_index,
        indent: indent_of(anchor),
    })
}

fn contiguous_trailing_comments(
    trivia: &[NodeOrToken<SyntaxNode, SyntaxToken>],
) -> Vec<SyntaxToken> {
    let mut chunks: Vec<Vec<SyntaxToken>> = vec![Vec::new()];
    let mut consecutive_newlines = 0;
    for el in trivia {
        match el {
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::COMMENT => {
                chunks.last_mut().unwrap().push(t.clone());
                consecutive_newlines = 0;
            }
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::NEWLINE => {
                consecutive_newlines += 1;
                if consecutive_newlines >= 2 && !chunks.last().unwrap().is_empty() {
                    chunks.push(Vec::new());
                }
            }
            _ => {} // whitespace: ignore for chunking
        }
    }
    chunks.pop().unwrap_or_default()
}

enum LeadingMode {
    /// Range covers the first preceding indent-WS through the anchor.
    /// Emission must reproduce that leading indent and re-establish the
    /// anchor's indent at the end.
    ReplaceFromLineStart,
    /// Range is empty (`[anchor_index..anchor_index)`), inserting before
    /// the anchor. The anchor owns its own leading indent (or has none),
    /// so emit indent on every comment line and omit a trailing indent.
    InsertBeforeAnchor,
}

fn build_leading_elements(
    text: Option<&str>,
    indent: &str,
    mode: LeadingMode,
) -> Vec<GreenElement> {
    let mut out: Vec<GreenElement> = Vec::new();
    let Some(text) = text else {
        return out;
    };
    let push_ws = |out: &mut Vec<GreenElement>| {
        if !indent.is_empty() {
            out.push(NodeOrToken::Token(mk_token(SyntaxKind::WHITESPACE, indent)));
        }
    };
    for line in text.split('\n') {
        // Both modes emit the indent on every comment line: the anchor
        // either owns its indent (so the comment lines must match its
        // column) or has none (push_ws is a no-op).
        push_ws(&mut out);
        let comment_text = if line.is_empty() {
            "#".to_string()
        } else {
            format!("# {line}")
        };
        out.push(NodeOrToken::Token(mk_token(
            SyntaxKind::COMMENT,
            &comment_text,
        )));
        out.push(NodeOrToken::Token(mk_token(SyntaxKind::NEWLINE, "\n")));
    }
    // Re-establish the anchor's indent so it lands at its original column.
    // Only needed when replacing from line start; in InsertBeforeAnchor
    // the anchor still owns its own indent after the splice point.
    if matches!(mode, LeadingMode::ReplaceFromLineStart) {
        push_ws(&mut out);
    }
    out
}

fn render_payload(comments: &[SyntaxToken]) -> String {
    comments
        .iter()
        .map(|t| strip_hash(t.text()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_hash(raw: &str) -> String {
    let after_hash = raw.strip_prefix('#').unwrap_or(raw);
    after_hash
        .strip_prefix(' ')
        .map_or(after_hash, |s| s)
        .to_string()
}

/// Read the same-line trailing comment directly attached to `entry`.
pub(crate) fn read_trailing(entry: &SyntaxNode) -> String {
    // Block entries hold their trailing comment as a direct child. Flow
    // entries can't (a comma may separate the value from the line end),
    // so the comment sits at the collection level just before the
    // line-ending NEWLINE — scan there.
    let token = if in_flow(entry) {
        find_trailing_comment_flow(entry)
    } else {
        find_trailing_comment_token(entry)
    };
    token.map(|t| strip_hash(t.text())).unwrap_or_default()
}

/// Find a flow entry's trailing COMMENT: the last COMMENT among the
/// siblings between the entry and the NEWLINE that ends its line.
fn find_trailing_comment_flow(entry: &SyntaxNode) -> Option<SyntaxToken> {
    let mut cur = entry.next_sibling_or_token();
    let mut found: Option<SyntaxToken> = None;
    while let Some(el) = cur {
        match el {
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::NEWLINE => break,
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::COMMENT => {
                found = Some(t.clone());
                cur = t.next_sibling_or_token();
            }
            NodeOrToken::Token(t)
                if matches!(t.kind(), SyntaxKind::WHITESPACE | SyntaxKind::COMMA) =>
            {
                cur = t.next_sibling_or_token();
            }
            _ => break,
        }
    }
    found
}

/// Set the same-line trailing comment on `entry`. Errors if `text`
/// contains a newline.
pub(crate) fn write_trailing(
    tree: &mut SyntaxTree,
    entry: &SyntaxNode,
    text: &str,
) -> Result<(), String> {
    if text.contains('\n') || text.contains('\r') {
        return Err("trailing comment cannot contain a newline".to_string());
    }
    if !can_write_trailing(entry) {
        return Err(
            "cannot attach a trailing comment: the entry is not followed by a newline \
             (e.g. a single-line flow item has no room for `# ...`)"
                .to_string(),
        );
    }
    set_trailing(tree, entry, Some(text));
    Ok(())
}

/// `write_leading` with the line-start guard applied. Block entries
/// always start a fresh line so the guard is a no-op for them; it only
/// rejects single-line flow entries that have no room for a comment
/// line above.
pub(crate) fn write_leading_guarded(
    tree: &mut SyntaxTree,
    anchor: &SyntaxNode,
    text: Option<&str>,
) -> Result<(), String> {
    // Clearing is always allowed (removing a comment can't break layout).
    if text.is_some() && !can_write_leading(anchor) {
        return Err(
            "cannot attach a leading comment: the entry is not preceded by a newline \
             (e.g. a single-line flow item has no room for a comment line above)"
                .to_string(),
        );
    }
    write_leading(tree, anchor, text);
    Ok(())
}

pub(crate) fn clear_trailing(tree: &mut SyntaxTree, entry: &SyntaxNode) {
    set_trailing(tree, entry, None);
}

/// True when a NEWLINE follows `entry` on its source line — i.e. the
/// entry is the last significant thing on its line, so a same-line
/// trailing comment can terminate cleanly. For flow entries an optional
/// COMMA separator may sit between the entry and the newline.
///
/// This is the write-legality guard for trailing comments: you cannot
/// add `# ...` to an entry that is followed by more content on the same
/// line (e.g. a single-line flow item) without breaking the document.
pub(crate) fn can_write_trailing(entry: &SyntaxNode) -> bool {
    line_ending_after(entry).is_some()
}

/// True when a NEWLINE precedes `entry` on its source line — i.e. the
/// entry starts a fresh line, so a leading comment block can sit above
/// it. Write-legality guard for leading comments.
pub(crate) fn can_write_leading(entry: &SyntaxNode) -> bool {
    let mut cur = entry.prev_sibling_or_token();
    while let Some(el) = cur {
        match el {
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::NEWLINE => return true,
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::WHITESPACE => {
                cur = t.prev_sibling_or_token();
            }
            _ => return false,
        }
    }
    // No preceding sibling at all: entry is first child of its parent.
    // It begins a fresh line only if the parent itself starts one, which
    // for a block collection at col 0 / a document root is true; the
    // walk-up via `is_first_meaningful_child` mirrors read placement.
    is_first_meaningful_child(entry)
}

/// If `entry`'s line ends in a NEWLINE (optionally past inline WS and a
/// single COMMA), return that NEWLINE token. Used both to guard writes
/// and to locate the splice point for a flow trailing comment.
fn line_ending_after(entry: &SyntaxNode) -> Option<SyntaxToken> {
    // First, a same-line trailing comment may already live inside the
    // entry (block layout); if so the line clearly ends after it.
    // Otherwise scan following siblings.
    let mut cur = entry.next_sibling_or_token();
    let mut seen_comma = false;
    while let Some(el) = cur {
        match el {
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::NEWLINE => return Some(t),
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::WHITESPACE => {
                cur = t.next_sibling_or_token();
            }
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::COMMENT => {
                cur = t.next_sibling_or_token();
            }
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::COMMA && !seen_comma => {
                seen_comma = true;
                cur = t.next_sibling_or_token();
            }
            _ => return None,
        }
    }
    None
}

fn find_trailing_comment_token(entry: &SyntaxNode) -> Option<SyntaxToken> {
    let mut last: Option<SyntaxToken> = None;
    for el in entry.children_with_tokens() {
        if let NodeOrToken::Token(t) = el
            && t.kind() == SyntaxKind::COMMENT
        {
            last = Some(t);
        }
    }
    last
}

/// True when `entry` lives directly inside a flow collection.
fn in_flow(entry: &SyntaxNode) -> bool {
    entry
        .parent()
        .map(|p| {
            matches!(
                p.kind(),
                SyntaxKind::FLOW_MAPPING | SyntaxKind::FLOW_SEQUENCE
            )
        })
        .unwrap_or(false)
}

fn set_trailing(tree: &mut SyntaxTree, entry: &SyntaxNode, new_text: Option<&str>) {
    if in_flow(entry) {
        set_trailing_flow(tree, entry, new_text);
        return;
    }
    let children: Vec<_> = entry.children_with_tokens().collect();
    let mut last_non_trivia_idx: Option<usize> = None;
    for (i, el) in children.iter().enumerate() {
        match el {
            NodeOrToken::Token(t) if t.kind().is_trivia() => {}
            _ => last_non_trivia_idx = Some(i),
        }
    }
    let Some(last_idx) = last_non_trivia_idx else {
        return;
    };

    let mut end = last_idx + 1;
    let mut has_existing_comment = false;
    while end < children.len() {
        match &children[end] {
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::COMMENT => {
                has_existing_comment = true;
                end += 1;
            }
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::WHITESPACE => {
                end += 1;
            }
            _ => break,
        }
    }
    let start = last_idx + 1;

    if !has_existing_comment && new_text.is_none() {
        return;
    }

    let mut replacement: Vec<GreenElement> = Vec::new();
    if let Some(text) = new_text {
        replacement.push(NodeOrToken::Token(mk_token(SyntaxKind::WHITESPACE, " ")));
        let comment_text = if text.is_empty() {
            "#".to_string()
        } else {
            format!("# {text}")
        };
        replacement.push(NodeOrToken::Token(mk_token(
            SyntaxKind::COMMENT,
            &comment_text,
        )));
    }

    let new_green = splice_children_green(entry, start..end, replacement);
    apply_edit(
        tree,
        TreeEdit::ReplaceNode {
            target: entry.clone(),
            new: new_green,
        },
    );
}

/// Set/clear a flow entry's trailing comment. The comment lands at the
/// flow-collection level, just before the NEWLINE that ends the entry's
/// line (past any COMMA separator), since the comma must stay adjacent
/// to the value. Caller has already verified `can_write_trailing`.
fn set_trailing_flow(tree: &mut SyntaxTree, entry: &SyntaxNode, new_text: Option<&str>) {
    let Some(newline) = line_ending_after(entry) else {
        return; // guard should have prevented this
    };
    let Some(parent) = entry.parent() else {
        return;
    };
    let nl_index = newline.index();

    // The comment + its leading WHITESPACE occupy the slots immediately
    // before the NEWLINE. Walk back over them to find the replace range.
    let children: Vec<_> = parent.children_with_tokens().collect();
    let mut start = nl_index;
    let mut has_existing_comment = false;
    while start > 0 {
        match &children[start - 1] {
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::COMMENT => {
                has_existing_comment = true;
                start -= 1;
            }
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::WHITESPACE => start -= 1,
            _ => break,
        }
    }

    if !has_existing_comment && new_text.is_none() {
        return;
    }

    let mut replacement: Vec<GreenElement> = Vec::new();
    if let Some(text) = new_text {
        replacement.push(NodeOrToken::Token(mk_token(SyntaxKind::WHITESPACE, " ")));
        let comment_text = if text.is_empty() {
            "#".to_string()
        } else {
            format!("# {text}")
        };
        replacement.push(NodeOrToken::Token(mk_token(
            SyntaxKind::COMMENT,
            &comment_text,
        )));
    }

    let new_green = splice_children_green(&parent, start..nl_index, replacement);
    apply_edit(
        tree,
        TreeEdit::ReplaceNode {
            target: parent,
            new: new_green,
        },
    );
}

#[cfg(test)]
mod tests {
    use crate::SyntaxTree;
    use crate::ast::{AstNode, Entry, FlowMapping, Node, Stream};

    fn first_flow_mapping(tree: &SyntaxTree) -> FlowMapping {
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let doc = stream.documents().next().unwrap();
        match doc.root_node().unwrap() {
            Node::FlowMapping(m) => m,
            other => panic!("expected flow mapping, got {other:?}"),
        }
    }

    const MULTILINE: &str = "{\n  host: localhost,\n  port: 8080\n}\n";

    #[test]
    fn flow_trailing_write_and_read_multiline() {
        let mut tree = crate::parse(MULTILINE).unwrap();
        let entry = first_flow_mapping(&tree).entries().next().unwrap();
        entry.set_trailing_comment(&mut tree, "the host").unwrap();
        assert_eq!(
            tree.emit(),
            "{\n  host: localhost, # the host\n  port: 8080\n}\n"
        );
        let tree2 = crate::parse(&tree.emit()).unwrap();
        let entry2 = first_flow_mapping(&tree2).entries().next().unwrap();
        assert_eq!(entry2.trailing_comment(), "the host");
    }

    #[test]
    fn flow_trailing_on_last_entry() {
        let mut tree = crate::parse(MULTILINE).unwrap();
        let entry = first_flow_mapping(&tree).entries().nth(1).unwrap();
        entry.set_trailing_comment(&mut tree, "the port").unwrap();
        assert_eq!(
            tree.emit(),
            "{\n  host: localhost,\n  port: 8080 # the port\n}\n"
        );
    }

    #[test]
    fn flow_trailing_clear() {
        let mut tree = crate::parse(MULTILINE).unwrap();
        let entry = first_flow_mapping(&tree).entries().next().unwrap();
        entry.set_trailing_comment(&mut tree, "x").unwrap();
        let entry = first_flow_mapping(&tree).entries().next().unwrap();
        entry.clear_trailing_comment(&mut tree);
        assert_eq!(tree.emit(), MULTILINE);
    }

    #[test]
    fn flow_leading_write_and_read() {
        let mut tree = crate::parse(MULTILINE).unwrap();
        let entry = first_flow_mapping(&tree).entries().nth(1).unwrap();
        entry
            .set_leading_comment(&mut tree, Some("about port"))
            .unwrap();
        assert_eq!(
            tree.emit(),
            "{\n  host: localhost,\n  # about port\n  port: 8080\n}\n"
        );
        let tree2 = crate::parse(&tree.emit()).unwrap();
        let entry2 = first_flow_mapping(&tree2).entries().nth(1).unwrap();
        assert_eq!(entry2.leading_comment(), "about port");
    }

    #[test]
    fn single_line_flow_trailing_rejected() {
        let mut tree = crate::parse("{a: 1, b: 2}\n").unwrap();
        let entry = first_flow_mapping(&tree).entries().next().unwrap();
        let err = entry.set_trailing_comment(&mut tree, "x").unwrap_err();
        assert!(err.contains("newline"), "got: {err}");
        assert_eq!(tree.emit(), "{a: 1, b: 2}\n");
    }

    #[test]
    fn single_line_flow_leading_rejected() {
        let mut tree = crate::parse("{a: 1, b: 2}\n").unwrap();
        let entry = first_flow_mapping(&tree).entries().nth(1).unwrap();
        let err = entry.set_leading_comment(&mut tree, Some("x")).unwrap_err();
        assert!(err.contains("newline"), "got: {err}");
        assert_eq!(tree.emit(), "{a: 1, b: 2}\n");
    }

    #[test]
    fn entry_trait_object_reads_key_value() {
        let tree = crate::parse(MULTILINE).unwrap();
        let entry = first_flow_mapping(&tree).entries().next().unwrap();
        let e: &dyn Entry = &entry;
        assert!(e.key().is_some());
        assert!(e.value().is_some());
    }

    #[test]
    fn flow_sequence_trailing() {
        let src = "[\n  1,\n  2\n]\n";
        let mut tree = crate::parse(src).unwrap();
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let doc = stream.documents().next().unwrap();
        let seq = match doc.root_node().unwrap() {
            Node::FlowSequence(s) => s,
            other => panic!("expected flow sequence, got {other:?}"),
        };
        let e = seq.entries().next().unwrap();
        e.set_trailing_comment(&mut tree, "one").unwrap();
        assert_eq!(tree.emit(), "[\n  1, # one\n  2\n]\n");
    }

    #[test]
    fn single_line_flow_clear_is_permissive() {
        // Clearing a (nonexistent) comment on a single-line flow entry
        // must not error, even though writing one would be rejected.
        let mut tree = crate::parse("{a: 1, b: 2}\n").unwrap();
        let entry = first_flow_mapping(&tree).entries().next().unwrap();
        entry.clear_trailing_comment(&mut tree); // no panic / no-op
        entry.set_leading_comment(&mut tree, None).unwrap(); // clear ok
        assert_eq!(tree.emit(), "{a: 1, b: 2}\n");
    }
}
