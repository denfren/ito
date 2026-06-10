use rowan::{GreenNode, GreenToken, Language, NodeOrToken};

use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken, YamlLang};

/// True iff `scalar` is the first node child of a mapping entry —
/// i.e. the entry's key.
pub(crate) fn is_key_position(scalar: &SyntaxNode) -> bool {
    let Some(parent) = scalar.parent() else {
        return false;
    };
    if !matches!(
        parent.kind(),
        SyntaxKind::BLOCK_MAPPING_ENTRY | SyntaxKind::FLOW_MAPPING_ENTRY
    ) {
        return false;
    }
    parent
        .children()
        .next()
        .map(|first| first == *scalar)
        .unwrap_or(false)
}

/// True iff `t` is a WHITESPACE token immediately before a NEWLINE or at EOF.
pub(crate) fn is_trailing_whitespace(t: &SyntaxToken) -> bool {
    match t.next_token() {
        Some(nxt) => nxt.kind() == SyntaxKind::NEWLINE,
        None => true,
    }
}

/// True iff `node`'s subtree contains a literal or folded block scalar token.
pub(crate) fn contains_block_scalar(node: &SyntaxNode) -> bool {
    for ev in node.preorder_with_tokens() {
        let rowan::WalkEvent::Enter(NodeOrToken::Token(t)) = ev else {
            continue;
        };
        if matches!(
            t.kind(),
            SyntaxKind::LITERAL_SCALAR | SyntaxKind::FOLDED_SCALAR
        ) {
            return true;
        }
    }
    false
}

/// True iff `n` is a `BLOCK_MAPPING_ENTRY` or `BLOCK_SEQUENCE_ENTRY`.
pub(crate) fn is_block_entry(n: &SyntaxNode) -> bool {
    matches!(
        n.kind(),
        SyntaxKind::BLOCK_MAPPING_ENTRY | SyntaxKind::BLOCK_SEQUENCE_ENTRY
    )
}

/// True iff `node` is the first non-trivia child of its parent.
pub(crate) fn is_first_meaningful_child(node: &SyntaxNode) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    for el in parent.children_with_tokens() {
        match &el {
            NodeOrToken::Node(n) => return n == node,
            NodeOrToken::Token(t) if t.kind().is_trivia() => continue,
            NodeOrToken::Token(_) => return false,
        }
    }
    false
}

/// The indentation string of `anchor`.
///
/// Per the uniform entry-indent ownership invariant, a line-start block
/// entry owns its leading indent WHITESPACE as its own first token; a
/// col-0 or mid-line entry owns none. So:
/// - if `anchor`'s first token is WHITESPACE, that *is* the indent;
/// - otherwise walk back through trivia to the preceding NEWLINE and
///   collect any WHITESPACE (covers nodes whose indent sits as a
///   preceding sibling, e.g. non-entry anchors).
///
/// Returns an empty string for a col-0 or mid-line anchor.
pub(crate) fn indent_of(anchor: &SyntaxNode) -> String {
    let Some(first) = anchor.first_token() else {
        return String::new();
    };
    // If the entry's own first token is a WHITESPACE (the leading indent
    // now lives inside the entry), use it directly.
    if first.kind() == SyntaxKind::WHITESPACE {
        return first.text().to_string();
    }
    let mut indent = String::new();
    let mut cur = first.prev_token();
    while let Some(t) = cur {
        if t.kind() == SyntaxKind::NEWLINE {
            break;
        }
        if !t.kind().is_trivia() {
            return String::new();
        }
        if t.kind() == SyntaxKind::WHITESPACE {
            indent.insert_str(0, t.text());
        }
        cur = t.prev_token();
    }
    indent
}

/// Observed 0-based column of the first character of `block`, measured
/// by walking backward through preceding tokens to find the last NEWLINE.
/// Recurses up the ancestor chain when the NEWLINE lives in a parent.
pub(crate) fn observed_block_col(block: &SyntaxNode) -> usize {
    observed_block_col_with_offset(block, 0)
}

/// Backward walk shared by the entry and ancestor-recursion cases.
///
/// `offset` is added to the inspected node's start before subtracting the
/// preceding NEWLINE's end. On the initial call it is `0` (so the node's
/// own absolute start is used). On the first hop into the ancestor chain
/// the caller folds the original block's intra-parent column into `offset`
/// (`block_start - parent_start`); further hops carry that same `offset`
/// unchanged, exactly as the previous two-function form did.
fn observed_block_col_with_offset(node: &SyntaxNode, offset: usize) -> usize {
    let node_start: usize = node.text_range().start().into();
    let mut t = node.prev_sibling_or_token();
    while let Some(el) = t {
        if let NodeOrToken::Token(tok) = &el
            && tok.kind() == SyntaxKind::NEWLINE
        {
            let nl_end: usize = tok.text_range().end().into();
            return (node_start + offset).saturating_sub(nl_end);
        }
        t = el.prev_sibling_or_token();
    }
    if let Some(parent) = node.parent() {
        // First ancestor hop folds the block's column within `parent`
        // into the offset; subsequent hops carry it unchanged.
        let next_offset = if offset == 0 {
            let parent_start: usize = parent.text_range().start().into();
            node_start.saturating_sub(parent_start)
        } else {
            offset
        };
        return observed_block_col_with_offset(&parent, next_offset);
    }
    // No NEWLINE and no parent: the node is the document root (start 0),
    // so `node_start + offset` reduces to the original `start` (offset 0)
    // or `offset` (ancestor recursion) — both forms coincide here.
    node_start + offset
}

/// Return a copy of `node`'s green subtree with every INTERIOR line (each
/// line after a NEWLINE token within the subtree) indented by `cols`
/// additional spaces. The text before the first interior NEWLINE (the
/// node's own first line) is left untouched — the caller supplies its
/// leading indent separately.
///
/// Concretely, for each line break the rule is:
/// - a WHITESPACE token that begins a fresh line (immediately follows a
///   NEWLINE) gets `cols` spaces prepended;
/// - a NEWLINE immediately followed by a non-whitespace token at column 0
///   gets a fresh WHITESPACE token of `cols` spaces inserted before that
///   token.
///
/// Blank lines (a NEWLINE immediately followed by another NEWLINE, or by
/// end of subtree) are left alone, so no trailing whitespace is created.
///
/// Lines whose content falls inside a literal (`|`) or folded (`>`) block
/// scalar are NOT reindented: their interior indentation is significant
/// and changing it would alter the scalar's value. (The value-model
/// builder never emits block scalars, so this only matters when the
/// helper is reused on hand-written or otherwise-built trees.)
///
/// `cols == 0` is a no-op (returns the green unchanged).
pub(crate) fn indent_green_continuations(node: &SyntaxNode, cols: usize) -> GreenNode {
    let green = node.green().into_owned();
    if cols == 0 {
        return green;
    }

    // Ranges (in the subtree's own coordinate space) covered by block
    // scalar tokens; lines starting inside any of these are left as-is.
    let base: usize = node.text_range().start().into();
    let scalar_ranges: Vec<(usize, usize)> = node
        .preorder_with_tokens()
        .filter_map(|ev| match ev {
            rowan::WalkEvent::Enter(NodeOrToken::Token(t))
                if matches!(
                    t.kind(),
                    SyntaxKind::LITERAL_SCALAR | SyntaxKind::FOLDED_SCALAR
                ) =>
            {
                let r = t.text_range();
                let start: usize = r.start().into();
                let end: usize = r.end().into();
                Some((start - base, end - base))
            }
            _ => None,
        })
        .collect();

    let mut ctx = ReindentCtx {
        cols,
        scalar_ranges,
        offset: 0,
        prev_was_newline: false,
    };
    ctx.rebuild_node(&green)
}

#[cfg(test)]
mod indent_tests {
    use super::*;

    /// Find the first node of `kind` in a freshly parsed `src`.
    fn node_of(src: &str, kind: SyntaxKind) -> (crate::SyntaxTree, SyntaxNode) {
        let tree = crate::parse(src).expect("parse");
        let node = tree
            .root()
            .descendants()
            .find(|n| n.kind() == kind)
            .unwrap_or_else(|| panic!("no {kind:?} in {src:?}"));
        (tree, node)
    }

    /// Emit the reindented green as standalone text.
    fn reindented_text(node: &SyntaxNode, cols: usize) -> String {
        let green = indent_green_continuations(node, cols);
        SyntaxNode::new_root(green).text().to_string()
    }

    #[test]
    fn reindents_multiline_map_entry_continuation() {
        // A two-line block mapping. First line bare, continuation +2.
        // The node's own text excludes the trailing NEWLINE sibling.
        let (_t, entry) = node_of("- k: 1\n  j: 2\n", SyntaxKind::BLOCK_SEQUENCE_ENTRY);
        assert_eq!(reindented_text(&entry, 2), "- k: 1\n    j: 2");
    }

    #[test]
    fn cols_zero_is_noop() {
        let (_t, entry) = node_of("- k: 1\n  j: 2\n", SyntaxKind::BLOCK_SEQUENCE_ENTRY);
        assert_eq!(reindented_text(&entry, 0), "- k: 1\n  j: 2");
    }

    #[test]
    fn inserts_indent_for_col0_continuation() {
        // A mapping whose second entry sits at column 0 (no leading
        // WHITESPACE token to grow): the helper INSERTS one.
        let (_t, map) = node_of("a: 1\nb: 2\n", SyntaxKind::BLOCK_MAPPING);
        assert_eq!(reindented_text(&map, 2), "a: 1\n  b: 2");
    }

    #[test]
    fn leaves_blank_lines_untouched() {
        // The blank line between the two entries must not gain trailing
        // whitespace.
        let (_t, map) = node_of("a: 1\n\nb: 2\n", SyntaxKind::BLOCK_MAPPING);
        assert_eq!(reindented_text(&map, 2), "a: 1\n\n  b: 2");
    }

    #[test]
    fn skips_block_scalar_interior() {
        // The literal scalar's interior lines carry significant
        // indentation and must NOT be touched. The entry's only
        // continuation lines ARE the scalar body, so reindenting is a
        // no-op here.
        let (_t, entry) = node_of("- |\n  a\n  b\n", SyntaxKind::BLOCK_SEQUENCE_ENTRY);
        assert_eq!(reindented_text(&entry, 2), "- |\n  a\n  b\n");
    }
}

struct ReindentCtx {
    cols: usize,
    /// Half-open ranges covered by block-scalar tokens (subtree coords).
    scalar_ranges: Vec<(usize, usize)>,
    /// Running text offset into the subtree as we walk tokens in order.
    offset: usize,
    /// Whether the immediately preceding token was a NEWLINE.
    prev_was_newline: bool,
}

impl ReindentCtx {
    fn in_block_scalar(&self, pos: usize) -> bool {
        self.scalar_ranges
            .iter()
            .any(|&(s, e)| pos >= s && pos < e)
    }

    fn rebuild_node(&mut self, green: &GreenNode) -> GreenNode {
        let mut children: Vec<NodeOrToken<GreenNode, GreenToken>> = Vec::new();
        let originals: Vec<_> = green.children().collect();
        for (ci, child) in originals.iter().enumerate() {
            match child {
                NodeOrToken::Node(n) => {
                    children.push(NodeOrToken::Node(self.rebuild_node(&(*n).to_owned())));
                }
                NodeOrToken::Token(t) => {
                    let kind = YamlLang::kind_from_raw(t.kind());
                    let text = t.text();
                    // A line begins here iff the previous token was a
                    // NEWLINE. Decide the edit before advancing offset.
                    let line_start = self.prev_was_newline;
                    let pos = self.offset;

                    // Insert leading indent for a line that starts with a
                    // non-whitespace token at column 0 (no WHITESPACE to
                    // grow), unless it is a blank line (NEWLINE) or sits in
                    // a block scalar.
                    if line_start
                        && self.cols > 0
                        && kind != SyntaxKind::WHITESPACE
                        && kind != SyntaxKind::NEWLINE
                        && !self.in_block_scalar(pos)
                    {
                        children.push(NodeOrToken::Token(GreenToken::new(
                            YamlLang::kind_to_raw(SyntaxKind::WHITESPACE),
                            &" ".repeat(self.cols),
                        )));
                    }

                    // Whether this token is the whole of a blank line:
                    // line-leading WHITESPACE whose next sibling token is a
                    // NEWLINE (or nothing). Padding it would add trailing
                    // space, so leave it untouched.
                    let trailing_only = matches!(
                        originals.get(ci + 1),
                        Some(NodeOrToken::Token(n)) if YamlLang::kind_from_raw(n.kind()) == SyntaxKind::NEWLINE
                    ) || originals.get(ci + 1).is_none();

                    if line_start
                        && kind == SyntaxKind::WHITESPACE
                        && self.cols > 0
                        && !trailing_only
                        && !self.in_block_scalar(pos)
                    {
                        // Grow an existing line-leading WHITESPACE.
                        let padded = format!("{}{}", " ".repeat(self.cols), text);
                        children.push(NodeOrToken::Token(GreenToken::new(
                            YamlLang::kind_to_raw(SyntaxKind::WHITESPACE),
                            &padded,
                        )));
                    } else {
                        children.push(NodeOrToken::Token((*t).to_owned()));
                    }

                    self.offset += text.len();
                    self.prev_was_newline = kind == SyntaxKind::NEWLINE;
                }
            }
        }
        GreenNode::new(green.kind(), children)
    }
}
