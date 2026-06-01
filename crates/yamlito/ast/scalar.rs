use crate::ast::nodes::{AstNode, NodeProperties};
use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarStyle {
    Plain,
    SingleQuoted,
    DoubleQuoted,
    Literal,
    Folded,
}

#[repr(transparent)]
#[derive(Debug, Clone)]
pub struct Scalar(pub(crate) SyntaxNode);

impl AstNode for Scalar {
    fn cast(n: SyntaxNode) -> Option<Self> {
        // A SCALAR carries a value token; a NULL_SCALAR is the zero-width
        // stand-in for an implicit null. Both are viewed as a `Scalar`
        // so callers don't special-case the empty slot.
        matches!(n.kind(), SyntaxKind::SCALAR | SyntaxKind::NULL_SCALAR).then_some(Self(n))
    }
    fn syntax(&self) -> &SyntaxNode {
        &self.0
    }
}

impl Scalar {
    /// True when this is the zero-width implicit-null node (no value
    /// token), as opposed to a scalar with an actual value.
    pub fn is_null(&self) -> bool {
        self.0.kind() == SyntaxKind::NULL_SCALAR
    }

    pub fn value_token(&self) -> Option<SyntaxToken> {
        self.0.children_with_tokens().find_map(|el| {
            el.into_token().filter(|t| {
                matches!(
                    t.kind(),
                    SyntaxKind::PLAIN_SCALAR
                        | SyntaxKind::SINGLE_QUOTED_SCALAR
                        | SyntaxKind::DOUBLE_QUOTED_SCALAR
                        | SyntaxKind::LITERAL_SCALAR
                        | SyntaxKind::FOLDED_SCALAR
                )
            })
        })
    }

    pub fn style(&self) -> Option<ScalarStyle> {
        self.value_token().map(|t| match t.kind() {
            SyntaxKind::PLAIN_SCALAR => ScalarStyle::Plain,
            SyntaxKind::SINGLE_QUOTED_SCALAR => ScalarStyle::SingleQuoted,
            SyntaxKind::DOUBLE_QUOTED_SCALAR => ScalarStyle::DoubleQuoted,
            SyntaxKind::LITERAL_SCALAR => ScalarStyle::Literal,
            SyntaxKind::FOLDED_SCALAR => ScalarStyle::Folded,
            _ => unreachable!(),
        })
    }

    pub fn raw_text(&self) -> Option<String> {
        self.value_token().map(|t| t.text().to_string())
    }

    pub fn properties(&self) -> Option<NodeProperties> {
        // NODE_PROPERTIES appears as a preceding sibling, not a child,
        // because properties attach before the node they decorate.
        preceding_properties(&self.0)
    }
}

pub(crate) fn preceding_properties(n: &SyntaxNode) -> Option<NodeProperties> {
    let mut cur = n.prev_sibling_or_token();
    while let Some(el) = cur {
        match el {
            rowan::NodeOrToken::Token(t) if t.kind().is_trivia() => {
                cur = t.prev_sibling_or_token();
            }
            rowan::NodeOrToken::Node(node) if node.kind() == SyntaxKind::NODE_PROPERTIES => {
                return NodeProperties::cast(node);
            }
            _ => return None,
        }
    }
    None
}
