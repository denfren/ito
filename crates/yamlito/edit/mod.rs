//! Structured tree edits.
//!
//! Edits mutate the tree in place via rowan's `splice_children`. Every
//! parsed tree is mutable (see `parser::parse`), and mutation uses
//! interior mutability — handles held by the caller automatically see
//! the result.
//!
//! # Edit kinds
//!
//! - `ReplaceToken { target, new }` — swap one token for a fresh one.
//!   Kinds must match.
//! - `ReplaceNode { target, new }` — swap a node subtree for a fresh
//!   green node. Kinds must match.
//! - `RemoveToken { target }` — detach a token from its parent.
//! - `RemoveNode { target }` — detach a node from its parent.
//!
//! Root replacement (`ReplaceNode` on a parentless node) is supported
//! by swapping the tree's root handle.

use rowan::{GreenNode, GreenToken, Language, NodeOrToken};

use crate::SyntaxTree;
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken, YamlLang};

mod build;
mod entry;

pub use build::{GreenElement, mk_node, mk_token};
pub use entry::{insert_map_entry, remove_map_entry};

#[derive(Debug, Clone)]
pub enum TreeEdit {
    ReplaceToken {
        target: SyntaxToken,
        new: GreenToken,
    },
    ReplaceNode {
        target: SyntaxNode,
        new: GreenNode,
    },
    RemoveToken {
        target: SyntaxToken,
    },
    RemoveNode {
        target: SyntaxNode,
    },
}

/// Apply a single edit to `tree` in place. The caller is responsible
/// for ensuring the edit's target still belongs to `tree` (i.e. that
/// no prior edit has invalidated it) and that any replacement's
/// `SyntaxKind` matches the target's.
pub fn apply_edit(tree: &mut SyntaxTree, edit: TreeEdit) {
    match edit {
        TreeEdit::ReplaceToken { target, new } => {
            assert_eq!(
                YamlLang::kind_from_raw(new.kind()),
                target.kind(),
                "ReplaceToken: kind must match",
            );
            let parent = target
                .parent()
                .expect("ReplaceToken: token must have a parent");
            let index = target.index();
            parent.splice_children(
                index..index + 1,
                std::iter::once(SyntaxElement::Token(wrap_token_for_splice(new))),
            );
        }
        TreeEdit::ReplaceNode { target, new } => {
            assert_eq!(
                YamlLang::kind_from_raw(new.kind()),
                target.kind(),
                "ReplaceNode: kind must match",
            );
            match target.parent() {
                None => {
                    // Root replacement — swap the tree's handle.
                    tree.set_root(SyntaxNode::new_root_mut(new));
                }
                Some(parent) => {
                    let index = target.index();
                    parent.splice_children(
                        index..index + 1,
                        std::iter::once(SyntaxElement::Node(SyntaxNode::new_root_mut(new))),
                    );
                }
            }
        }
        TreeEdit::RemoveToken { target } => {
            target.detach();
        }
        TreeEdit::RemoveNode { target } => {
            target.detach();
        }
    }
}

/// Wrap a fresh `GreenToken` in a throwaway single-token parent so we
/// can obtain a mutable `SyntaxToken` suitable for `splice_children`.
/// The wrapper parent is immediately discarded when the returned token
/// is attached elsewhere.
fn wrap_token_for_splice(tok: GreenToken) -> SyntaxToken {
    // Any node kind works as the transient wrapper; STREAM is arbitrary
    // but always valid. The wrapper is detached as soon as splice_children
    // reattaches the token to its real parent.
    let wrapper_green = GreenNode::new(
        YamlLang::kind_to_raw(SyntaxKind::STREAM),
        [NodeOrToken::Token(tok)],
    );
    let wrapper = SyntaxNode::new_root_mut(wrapper_green);
    wrapper
        .first_token()
        .expect("single-token wrapper has a token")
}

/// Build a new green node from `parent` with `parent.children_with_tokens()[range]`
/// replaced by `new`. Useful for callers who need to produce a green
/// replacement for `ReplaceNode` when structural changes go beyond a
/// single child.
pub fn splice_children_green(
    parent: &SyntaxNode,
    range: std::ops::Range<usize>,
    new: Vec<GreenElement>,
) -> GreenNode {
    let kind = YamlLang::kind_to_raw(parent.kind());
    let mut children: Vec<GreenElement> = Vec::new();
    let count = parent.children_with_tokens().count();
    for (i, ch) in parent.children_with_tokens().enumerate() {
        if i == range.start {
            children.extend(new.iter().cloned());
        }
        if !range.contains(&i) {
            children.push(element_to_green(ch));
        }
    }
    if range.start >= count {
        children.extend(new);
    }
    GreenNode::new(kind, children)
}

fn element_to_green(el: SyntaxElement) -> GreenElement {
    match el {
        NodeOrToken::Node(n) => NodeOrToken::Node(n.green().into_owned()),
        NodeOrToken::Token(t) => NodeOrToken::Token(t.green().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AstNode, Node, Stream};
    use crate::syntax::SyntaxKind::*;

    fn all_scalars_in(root: &SyntaxNode) -> Vec<crate::ast::Scalar> {
        let mut out = Vec::new();
        fn walk(n: &SyntaxNode, out: &mut Vec<crate::ast::Scalar>) {
            if let Some(s) = crate::ast::Scalar::cast(n.clone()) {
                out.push(s);
            }
            for c in n.children() {
                walk(&c, out);
            }
        }
        walk(root, &mut out);
        out
    }

    /// Parse input and return the first BLOCK_MAPPING_ENTRY.
    fn first_entry(src: &str) -> (SyntaxTree, SyntaxNode) {
        let tree = crate::parse(src).expect("parse");
        let entry = tree
            .root()
            .descendants()
            .find(|n| n.kind() == BLOCK_MAPPING_ENTRY)
            .expect("no BLOCK_MAPPING_ENTRY found");
        (tree, entry)
    }

    #[test]
    fn splice_children_green_inserts_before_index() {
        let (_tree, entry) = first_entry("k: v\n");
        let new_ws = NodeOrToken::Token(mk_token(WHITESPACE, "  "));
        let green = splice_children_green(&entry, 0..0, vec![new_ws]);
        let new_root = SyntaxNode::new_root(green);
        assert_eq!(new_root.text(), "  k: v");
    }

    #[test]
    fn splice_children_green_replaces_range() {
        let (_tree, entry) = first_entry("k: v\n");
        // Replace `COLON WS` with `=`. Indices 1..3 span those tokens
        // because the parser emits: [SCALAR(k), COLON, WS, SCALAR(v)].
        let repl = NodeOrToken::Token(mk_token(COLON, "="));
        let green = splice_children_green(&entry, 1..3, vec![repl]);
        let new_root = SyntaxNode::new_root(green);
        assert_eq!(new_root.text(), "k=v");
    }

    #[test]
    fn splice_children_green_appends_at_end() {
        let (_tree, entry) = first_entry("k: v\n");
        let end = entry.children_with_tokens().count();
        let extra = NodeOrToken::Token(mk_token(WHITESPACE, "!"));
        let green = splice_children_green(&entry, end..end, vec![extra]);
        let new_root = SyntaxNode::new_root(green);
        assert_eq!(new_root.text(), "k: v!");
    }

    #[test]
    #[should_panic(expected = "ReplaceToken: kind must match")]
    fn replace_token_rejects_kind_mismatch() {
        let (mut tree, entry) = first_entry("k: v\n");
        let target = entry
            .children_with_tokens()
            .find_map(|e| match e {
                NodeOrToken::Token(t) if t.kind() == COLON => Some(t),
                _ => None,
            })
            .unwrap();
        apply_edit(
            &mut tree,
            TreeEdit::ReplaceToken {
                target,
                new: mk_token(WHITESPACE, " "),
            },
        );
    }

    #[test]
    #[should_panic(expected = "ReplaceNode: kind must match")]
    fn replace_node_rejects_kind_mismatch() {
        let (mut tree, entry) = first_entry("k: v\n");
        let target = entry.children().find(|c| c.kind() == SCALAR).unwrap();
        let new = mk_node(FLOW_MAPPING, [NodeOrToken::Token(mk_token(L_BRACE, "{"))]);
        apply_edit(&mut tree, TreeEdit::ReplaceNode { target, new });
    }

    #[test]
    fn remove_token_detaches_from_parent() {
        let (mut tree, entry) = first_entry("k: v\n");
        let target = entry
            .children_with_tokens()
            .find_map(|e| match e {
                NodeOrToken::Token(t) if t.kind() == WHITESPACE => Some(t),
                _ => None,
            })
            .unwrap();
        apply_edit(&mut tree, TreeEdit::RemoveToken { target });
        assert_eq!(tree.emit(), "k:v\n");
    }

    #[test]
    fn replace_token_swaps_scalar_text() {
        let mut tree = crate::parse("a: hello\n").expect("parse");
        let scalar = all_scalars_in(tree.root())
            .into_iter()
            .find(|s| s.decoded().ok().as_deref() == Some("hello"))
            .expect("hello scalar");
        let token = scalar.value_token().expect("value token");
        apply_edit(
            &mut tree,
            TreeEdit::ReplaceToken {
                new: mk_token(token.kind(), "world"),
                target: token,
            },
        );
        assert_eq!(tree.emit(), "a: world\n");
    }

    #[test]
    fn replace_token_plain_to_quoted_form() {
        let mut tree = crate::parse("a: yes\n").expect("parse");
        let scalar = all_scalars_in(tree.root())
            .into_iter()
            .find(|s| s.decoded().ok().as_deref() == Some("yes"))
            .expect("yes scalar");
        // Swap the PLAIN_SCALAR for a SINGLE_QUOTED_SCALAR — different
        // kind, so we must replace the enclosing SCALAR node instead.
        let new_node = mk_node(
            SyntaxKind::SCALAR,
            [NodeOrToken::Token(mk_token(
                SyntaxKind::SINGLE_QUOTED_SCALAR,
                "'yes'",
            ))],
        );
        apply_edit(
            &mut tree,
            TreeEdit::ReplaceNode {
                target: scalar.syntax().clone(),
                new: new_node,
            },
        );
        assert_eq!(tree.emit(), "a: 'yes'\n");
    }

    #[test]
    fn replace_node_swaps_whole_scalar() {
        let mut tree = crate::parse("key: old\n").expect("parse");
        let scalar = all_scalars_in(tree.root())
            .into_iter()
            .find(|s| s.decoded().ok().as_deref() == Some("old"))
            .expect("old scalar");
        let new = mk_node(
            SyntaxKind::SCALAR,
            [NodeOrToken::Token(mk_token(
                SyntaxKind::PLAIN_SCALAR,
                "new",
            ))],
        );
        apply_edit(
            &mut tree,
            TreeEdit::ReplaceNode {
                target: scalar.syntax().clone(),
                new,
            },
        );
        assert_eq!(tree.emit(), "key: new\n");
    }

    #[test]
    fn mutated_tree_re_parses_cleanly() {
        let mut tree = crate::parse("a: hello\n").expect("parse");
        let scalar = all_scalars_in(tree.root())
            .into_iter()
            .find(|s| s.decoded().ok().as_deref() == Some("hello"))
            .expect("hello");
        let token = scalar.value_token().expect("value token");
        apply_edit(
            &mut tree,
            TreeEdit::ReplaceToken {
                new: mk_token(token.kind(), "world"),
                target: token,
            },
        );
        let emitted = tree.emit();
        crate::parse(&emitted).expect("reparse");
        assert_eq!(emitted, "a: world\n");
    }

    #[test]
    fn replace_node_on_root() {
        let mut tree = crate::parse("a: 1\n").expect("parse");
        // Build a fresh STREAM with different content by cloning green
        // from a fresh parse.
        let other = crate::parse("b: 2\n").expect("parse");
        let other_green: rowan::GreenNode = other.root().green().into_owned();
        // Kinds match (both STREAM) so the assertion holds.
        let target = tree.root().clone();
        apply_edit(
            &mut tree,
            TreeEdit::ReplaceNode {
                target,
                new: other_green,
            },
        );
        assert_eq!(tree.emit(), "b: 2\n");
    }

    #[test]
    fn stream_cast_after_edit() {
        let mut tree = crate::parse("a: hello\n").expect("parse");
        let scalar = all_scalars_in(tree.root())
            .into_iter()
            .find(|s| s.decoded().ok().as_deref() == Some("hello"))
            .expect("hello");
        let token = scalar.value_token().expect("v");
        apply_edit(
            &mut tree,
            TreeEdit::ReplaceToken {
                new: mk_token(token.kind(), "world"),
                target: token,
            },
        );
        let stream = Stream::cast(tree.root().clone()).expect("stream");
        let doc = stream.documents().next().expect("doc");
        let root = doc.root_node().expect("root node");
        if let Node::BlockMapping(m) = root {
            use crate::ast::Mapping;
            let v = m.get("a").expect("a");
            if let Node::Scalar(s) = v {
                assert_eq!(s.decoded().ok().as_deref(), Some("world"));
            } else {
                panic!("value not a scalar");
            }
        } else {
            panic!("root not a block mapping");
        }
    }
}
