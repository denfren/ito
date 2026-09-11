//! Structured tree edits.
//!
//! Edits rebuild the tree immutably: the target's replacement is
//! spliced into its parent's green node, and that rebuild is propagated
//! up the ancestor chain to a fresh root, which is swapped into the
//! `SyntaxTree`.
//!
//! Because rowan trees are persistent, an edit invalidates every
//! `SyntaxNode`/`SyntaxToken` handle taken before it: the handles still
//! point into the *old* root. Callers routinely collect a batch of
//! targets up front and then apply edits one at a time, so `apply_edit`
//! re-resolves each target against the current root by its root-relative
//! child-index path (see `index_path`). That keeps stale-but-structurally
//! valid handles working, exactly as the old in-place `splice_children`
//! did.
//!
//! The path is only stable if edits preserve the indices of the nodes
//! that later edits target. `ReplaceToken`/`ReplaceNode` are 1-for-1 and
//! always safe; the `Remove*` variants shift the indices of their later
//! siblings, so batches that remove must be applied back-to-front (as
//! `fixers` does by collecting in document order and removing tokens
//! whose siblings are not themselves targets).
//!
//! Index paths are therefore a best-effort rescue, not an identity: a
//! path that survived a removal can name a *different* node, and one
//! that no longer names a node at all makes `apply_edit` drop the edit
//! (see `node_at_path`) rather than rewrite an unrelated one. A caller
//! that needs a handle to stay meaningful across arbitrary edits should
//! re-descend from `tree.root()` by key/index instead — which is what
//! `ito`'s `yaml.edit` cursor does.
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
use crate::syntax::{SyntaxElement, SyntaxNode, SyntaxToken, YamlLang};

mod build;
mod collection;
mod entry;
mod seq;

pub use build::{GreenElement, mk_node, mk_token};
pub use entry::{insert_map_entry, remove_map_entry};
pub use seq::{insert_seq_item, push_seq_item, remove_seq_item};

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
            splice_into(
                tree,
                &parent,
                index..index + 1,
                vec![NodeOrToken::Token(new)],
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
                    tree.set_root(SyntaxNode::new_root(new));
                }
                Some(parent) => {
                    let index = target.index();
                    splice_into(
                        tree,
                        &parent,
                        index..index + 1,
                        vec![NodeOrToken::Node(new)],
                    );
                }
            }
        }
        TreeEdit::RemoveToken { target } => {
            let parent = target
                .parent()
                .expect("RemoveToken: token must have a parent");
            let index = target.index();
            splice_into(tree, &parent, index..index + 1, Vec::new());
        }
        TreeEdit::RemoveNode { target } => match target.parent() {
            None => panic!("RemoveNode: cannot remove the tree root"),
            Some(parent) => {
                let index = target.index();
                splice_into(tree, &parent, index..index + 1, Vec::new());
            }
        },
    }
}

/// The child indices leading from the tree root down to `node`, root
/// first. `[]` means `node` is the root.
///
/// Indices are into `children_with_tokens()`, i.e. they count tokens as
/// well as nodes — the same space `splice_children_green` operates in.
pub(crate) fn index_path(node: &SyntaxNode) -> Vec<usize> {
    let mut path = Vec::new();
    let mut cur = node.clone();
    while let Some(parent) = cur.parent() {
        path.push(cur.index());
        cur = parent;
    }
    path.reverse();
    path
}

/// Re-resolve `path` (from `index_path`) against `root`, or `None` when
/// it no longer names a node.
///
/// A path taken before a *structural* edit can outlive the shape it
/// described: `Remove*` shifts its later siblings down, so an index that
/// once named a node may now name a token, or be past the end. Reporting
/// that as `None` rather than panicking is what keeps a stale handle from
/// aborting the process — the caller decides what a vanished target means.
pub(crate) fn node_at_path(root: &SyntaxNode, path: &[usize]) -> Option<SyntaxNode> {
    let mut cur = root.clone();
    for &i in path {
        cur = match cur.children_with_tokens().nth(i) {
            Some(NodeOrToken::Node(n)) => n,
            _ => return None,
        };
    }
    Some(cur)
}

/// Replace `parent.children_with_tokens()[range]` with `new`, rebuilding
/// every ancestor up to a fresh root and installing it in `tree`.
///
/// `parent` may be a handle into an older revision of the tree; it is
/// re-resolved by index path against the current root first.
fn splice_into(
    tree: &mut SyntaxTree,
    parent: &SyntaxNode,
    range: std::ops::Range<usize>,
    new: Vec<GreenElement>,
) {
    let path = index_path(parent);
    // A parent from a superseded revision may no longer exist at its old
    // index path. Dropping the edit is the only safe answer: applying it
    // where the stale path now points would rewrite an unrelated node.
    let Some(live) = node_at_path(tree.root(), &path) else {
        return;
    };
    let mut green = splice_children_green(&live, range, new);

    // Walk back up, rebuilding each ancestor around the new child green.
    let mut cur = live;
    while let Some(ancestor) = cur.parent() {
        let idx = cur.index();
        green = splice_children_green(&ancestor, idx..idx + 1, vec![NodeOrToken::Node(green)]);
        cur = ancestor;
    }
    tree.set_root(SyntaxNode::new_root(green));
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
        NodeOrToken::Node(n) => NodeOrToken::Node(n.green().to_owned()),
        NodeOrToken::Token(t) => NodeOrToken::Token(t.green().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AstNode, Node, Stream};
    use crate::syntax::SyntaxKind;
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
        // Entry children are [SCALAR(k), COLON, WS, SCALAR(v)]; replace
        // `COLON WS` (1..3) with `=`.
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

    /// A handle taken before a removal can outlive the shape it named.
    /// Editing through it must not abort the process — `node_at_path`
    /// reports the miss and the edit is dropped.
    #[test]
    fn edit_through_stale_target_does_not_panic() {
        let mut tree = crate::parse("a: 1\nb: 2\nc: 3\n").expect("parse");
        let stale = all_scalars_in(tree.root())
            .into_iter()
            .find(|s| s.decoded().ok().as_deref() == Some("3"))
            .expect("c scalar");
        let token = stale.value_token().expect("value token");

        // Remove the first entry, shifting every later sibling's index.
        let map = tree
            .root()
            .descendants()
            .find(|n| n.kind() == SyntaxKind::BLOCK_MAPPING)
            .expect("block mapping");
        remove_map_entry(&mut tree, &map, 0);

        // The stale token's index path no longer names its old node.
        apply_edit(
            &mut tree,
            TreeEdit::ReplaceToken {
                new: mk_token(token.kind(), "99"),
                target: token,
            },
        );
        // No panic; the tree is still well-formed and re-parses.
        crate::parse(&tree.emit()).expect("reparse");
    }

    #[test]
    fn replace_node_on_root() {
        let mut tree = crate::parse("a: 1\n").expect("parse");
        // Build a fresh STREAM with different content by cloning green
        // from a fresh parse.
        let other = crate::parse("b: 2\n").expect("parse");
        let other_green: rowan::GreenNode = other.root().green().to_owned();
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
