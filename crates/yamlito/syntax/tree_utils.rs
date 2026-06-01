use rowan::NodeOrToken;

use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

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
    let start: usize = block.text_range().start().into();
    let mut col = start;
    let mut t = block.prev_sibling_or_token();
    while let Some(el) = t {
        match &el {
            NodeOrToken::Token(tok) if tok.kind() == SyntaxKind::NEWLINE => {
                let nl_end: usize = tok.text_range().end().into();
                return start.saturating_sub(nl_end);
            }
            _ => {}
        }
        t = el.prev_sibling_or_token();
    }
    if let Some(parent) = block.parent() {
        let parent_start: usize = parent.text_range().start().into();
        col = col.saturating_sub(parent_start);
        return observed_block_col_from_ancestor(&parent, col);
    }
    col
}

pub(crate) fn observed_block_col_from_ancestor(
    node: &SyntaxNode,
    accumulated_offset: usize,
) -> usize {
    let node_start: usize = node.text_range().start().into();
    let mut t = node.prev_sibling_or_token();
    while let Some(el) = t {
        if let NodeOrToken::Token(tok) = &el
            && tok.kind() == SyntaxKind::NEWLINE
        {
            let nl_end: usize = tok.text_range().end().into();
            return (node_start + accumulated_offset).saturating_sub(nl_end);
        }
        t = el.prev_sibling_or_token();
    }
    if let Some(parent) = node.parent() {
        return observed_block_col_from_ancestor(&parent, accumulated_offset);
    }
    accumulated_offset
}
