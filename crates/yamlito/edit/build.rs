//! Tiny helpers for constructing green nodes and tokens without
//! exposing rowan's `Language::kind_to_raw` ceremony at call sites.

use rowan::{GreenNode, GreenToken, Language, NodeOrToken};

use crate::syntax::{SyntaxKind, YamlLang};

pub type GreenElement = NodeOrToken<GreenNode, GreenToken>;

pub fn mk_token(kind: SyntaxKind, text: &str) -> GreenToken {
    GreenToken::new(YamlLang::kind_to_raw(kind), text)
}

pub fn mk_node<I>(kind: SyntaxKind, children: I) -> GreenNode
where
    I: IntoIterator<Item = GreenElement>,
    I::IntoIter: ExactSizeIterator,
{
    GreenNode::new(YamlLang::kind_to_raw(kind), children)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{TreeEdit, apply_edit};
    use crate::syntax::SyntaxKind::*;

    #[test]
    fn mk_token_round_trips_kind_when_swapped_into_parsed_tree() {
        let mut tree = crate::parse("k: hello\n").expect("parse");
        let target = tree
            .root()
            .descendants_with_tokens()
            .find_map(|e| match e {
                NodeOrToken::Token(t) if t.kind() == PLAIN_SCALAR && t.text() == "hello" => Some(t),
                _ => None,
            })
            .unwrap();
        apply_edit(
            &mut tree,
            TreeEdit::ReplaceToken {
                target,
                new: mk_token(PLAIN_SCALAR, "world"),
            },
        );
        assert_eq!(tree.emit(), "k: world\n");
    }

    #[test]
    fn mk_node_replaces_parsed_scalar_with_built_equivalent() {
        let mut tree = crate::parse("k: hello\n").expect("parse");
        let target = tree
            .root()
            .descendants()
            .find(|n| n.kind() == SCALAR && n.text() == "hello")
            .unwrap();
        let new = mk_node(
            SCALAR,
            [NodeOrToken::Token(mk_token(SINGLE_QUOTED_SCALAR, "'hi'"))],
        );
        apply_edit(&mut tree, TreeEdit::ReplaceNode { target, new });
        assert_eq!(tree.emit(), "k: 'hi'\n");
    }
}
