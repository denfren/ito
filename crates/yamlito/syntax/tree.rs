use rowan::{NodeOrToken, WalkEvent};

use crate::syntax::SyntaxNode;

#[derive(Debug, Clone)]
pub struct SyntaxTree {
    root: SyntaxNode,
}

impl SyntaxTree {
    pub fn new(root: SyntaxNode) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &SyntaxNode {
        &self.root
    }

    /// Swap the root node. Used by the edit engine after a mutation.
    pub fn set_root(&mut self, root: SyntaxNode) {
        self.root = root;
    }

    /// Serialize the tree to text by walking its tokens.
    pub fn emit(&self) -> String {
        let mut out = String::new();
        for ev in self.root.preorder_with_tokens() {
            if let WalkEvent::Enter(NodeOrToken::Token(t)) = ev {
                out.push_str(t.text());
            }
        }
        out
    }
}
