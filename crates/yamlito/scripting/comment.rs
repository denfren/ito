//! Adapter: map a Rhai cursor's `SyntaxNode` to the nearest AST node
//! that owns comments (`BlockMappingEntry`, `BlockSequenceEntry`,
//! `Document`) and dispatch the call. All per-variant quirks live on
//! those AST types.
//!
//! Cursors that resolve to no block-style anchor — i.e. cursors sitting
//! inside a `FLOW_MAPPING` or `FLOW_SEQUENCE` — error on write and
//! return empty on read. Block-style YAML is the only form where
//! line-comments round-trip cleanly, and auto-reflowing a flow
//! construct to attach one is out of scope.

use crate::SyntaxTree;
use crate::ast::{AstNode, BlockMappingEntry, BlockSequenceEntry, Document};
use crate::syntax::{SyntaxKind, SyntaxNode};

/// Enclosing anchor for comment operations. The variants match the
/// three AST types that expose comment methods.
enum Anchor {
    MapEntry(BlockMappingEntry),
    SeqEntry(BlockSequenceEntry),
    Doc(Document),
}

impl Anchor {
    fn find(cursor_node: &SyntaxNode) -> Option<Self> {
        // Walk up looking for the first block-style entry or document.
        let mut cur = Some(cursor_node.clone());
        while let Some(n) = cur {
            match n.kind() {
                SyntaxKind::BLOCK_MAPPING_ENTRY => {
                    return BlockMappingEntry::cast(n).map(Anchor::MapEntry);
                }
                SyntaxKind::BLOCK_SEQUENCE_ENTRY => {
                    return BlockSequenceEntry::cast(n).map(Anchor::SeqEntry);
                }
                SyntaxKind::DOCUMENT => {
                    return Document::cast(n).map(Anchor::Doc);
                }
                SyntaxKind::FLOW_MAPPING
                | SyntaxKind::FLOW_SEQUENCE
                | SyntaxKind::FLOW_MAPPING_ENTRY
                | SyntaxKind::FLOW_SEQUENCE_ENTRY => {
                    // Crossing into a flow construct means we can't
                    // attach a line comment without reflowing; bail.
                    return None;
                }
                _ => {}
            }
            cur = n.parent();
        }
        None
    }
}

pub fn read_leading(cursor_node: &SyntaxNode) -> String {
    match Anchor::find(cursor_node) {
        Some(Anchor::MapEntry(e)) => e.leading_comment(),
        Some(Anchor::SeqEntry(e)) => e.leading_comment(),
        Some(Anchor::Doc(d)) => d.leading_comment(),
        None => String::new(),
    }
}

pub fn write_leading(
    tree: &mut SyntaxTree,
    cursor_node: &SyntaxNode,
    text: &str,
) -> Result<(), String> {
    match Anchor::find(cursor_node) {
        Some(Anchor::MapEntry(e)) => e.set_leading_comment(tree, Some(text)),
        Some(Anchor::SeqEntry(e)) => e.set_leading_comment(tree, Some(text)),
        Some(Anchor::Doc(d)) => {
            d.set_leading_comment(tree, Some(text));
            Ok(())
        }
        None => Err(
            "cannot attach a comment inside a flow-style mapping or sequence; \
             convert to block form first"
                .to_string(),
        ),
    }
}

pub fn clear_leading(tree: &mut SyntaxTree, cursor_node: &SyntaxNode) {
    match Anchor::find(cursor_node) {
        // Clearing block leading comments never fails the line guard.
        Some(Anchor::MapEntry(e)) => {
            let _ = e.set_leading_comment(tree, None);
        }
        Some(Anchor::SeqEntry(e)) => {
            let _ = e.set_leading_comment(tree, None);
        }
        Some(Anchor::Doc(d)) => d.set_leading_comment(tree, None),
        None => {}
    }
}

pub fn read_trailing(cursor_node: &SyntaxNode) -> String {
    match Anchor::find(cursor_node) {
        Some(Anchor::MapEntry(e)) => e.trailing_comment(),
        Some(Anchor::SeqEntry(e)) => e.trailing_comment(),
        // Documents don't carry same-line trailing comments in the
        // entry sense; return empty.
        _ => String::new(),
    }
}

pub fn write_trailing(
    tree: &mut SyntaxTree,
    cursor_node: &SyntaxNode,
    text: &str,
) -> Result<(), String> {
    match Anchor::find(cursor_node) {
        Some(Anchor::MapEntry(e)) => e.set_trailing_comment(tree, text),
        Some(Anchor::SeqEntry(e)) => e.set_trailing_comment(tree, text),
        Some(Anchor::Doc(_)) => {
            Err("documents cannot carry a same-line trailing comment".to_string())
        }
        None => Err(
            "cannot attach a trailing comment inside a flow-style mapping or sequence".to_string(),
        ),
    }
}

pub fn clear_trailing(tree: &mut SyntaxTree, cursor_node: &SyntaxNode) {
    match Anchor::find(cursor_node) {
        Some(Anchor::MapEntry(e)) => e.clear_trailing_comment(tree),
        Some(Anchor::SeqEntry(e)) => e.clear_trailing_comment(tree),
        _ => {}
    }
}
