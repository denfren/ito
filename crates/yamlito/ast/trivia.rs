use std::vec::IntoIter;

use crate::syntax::{SyntaxNode, SyntaxToken};

/// Trivia tokens (whitespace/newline/comment) immediately preceding `node`,
/// in document order. Walks back from `node`'s first token until a
/// non-trivia token, the start of file, or the boundary of `node`'s
/// parent is reached — the last of which prevents absorbing a previous
/// sibling's trailing trivia.
pub fn leading_trivia(node: &SyntaxNode) -> IntoIter<SyntaxToken> {
    let mut collected: Vec<SyntaxToken> = Vec::new();
    let parent_range = node.parent().map(|p| p.text_range());
    if let Some(first) = node.first_token() {
        let mut cur = first.prev_token();
        while let Some(t) = cur {
            if !t.kind().is_trivia() {
                break;
            }
            if let Some(pr) = parent_range
                && !pr.contains_range(t.text_range())
            {
                break;
            }
            collected.push(t.clone());
            cur = t.prev_token();
        }
    }
    collected.reverse();
    collected.into_iter()
}

/// Trivia tokens at the tail of `node`, in document order. Walks back
/// from `node`'s last token across contiguous trivia tokens that are
/// still within the node, stopping at the first non-trivia token or
/// when leaving the node. Returns the collected trivia in document
/// order (earliest first).
///
/// This captures the same-line trailing whitespace/comment that the
/// parser attaches to its preceding entry/node.
pub fn trailing_trivia(node: &SyntaxNode) -> IntoIter<SyntaxToken> {
    let mut collected: Vec<SyntaxToken> = Vec::new();
    let range = node.text_range();
    if let Some(last) = node.last_token() {
        let mut cur = Some(last);
        while let Some(t) = cur {
            if !t.kind().is_trivia() {
                break;
            }
            if !range.contains_range(t.text_range()) {
                break;
            }
            collected.push(t.clone());
            cur = t.prev_token();
        }
    }
    collected.reverse();
    collected.into_iter()
}
