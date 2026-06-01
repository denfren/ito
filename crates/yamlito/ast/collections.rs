//! S-A6: shared `Mapping` and `Sequence` traits for block- and flow-
//! style collections, plus convenience accessors (`.get`, `.keys`, etc.).

use crate::SyntaxTree;
use crate::ast::nodes::{
    BlockMapping, BlockMappingEntry, BlockSequence, BlockSequenceEntry, FlowMapping,
    FlowMappingEntry, FlowSequence, FlowSequenceEntry, Node,
};
use crate::ast::value::Value;

/// Uniform view over a mapping/sequence entry of either style. Sequence
/// entries have no key, so `key()` returns `None` for them.
///
/// Comment accessors follow a line-based model that is identical for
/// block and flow: a *trailing* comment is on the same line, after the
/// value; a *leading* comment is on its own line, before the entry.
/// Writes are rejected when the layout has no room for the comment
/// (e.g. a single-line flow item): `set_trailing_comment` requires a
/// newline after the entry, `set_leading_comment` a newline before it.
pub trait Entry {
    fn key(&self) -> Option<Node>;
    fn value(&self) -> Option<Node>;
    fn leading_comment(&self) -> String;
    fn set_leading_comment(&self, tree: &mut SyntaxTree, text: Option<&str>) -> Result<(), String>;
    fn trailing_comment(&self) -> String;
    fn set_trailing_comment(&self, tree: &mut SyntaxTree, text: &str) -> Result<(), String>;
    fn clear_trailing_comment(&self, tree: &mut SyntaxTree);
}

macro_rules! impl_entry {
    ($ty:ty, key = $key:expr) => {
        impl Entry for $ty {
            fn key(&self) -> Option<Node> {
                #[allow(clippy::redundant_closure_call)]
                $key(self)
            }
            fn value(&self) -> Option<Node> {
                <$ty>::value(self)
            }
            fn leading_comment(&self) -> String {
                <$ty>::leading_comment(self)
            }
            fn set_leading_comment(
                &self,
                tree: &mut SyntaxTree,
                text: Option<&str>,
            ) -> Result<(), String> {
                <$ty>::set_leading_comment(self, tree, text)
            }
            fn trailing_comment(&self) -> String {
                <$ty>::trailing_comment(self)
            }
            fn set_trailing_comment(
                &self,
                tree: &mut SyntaxTree,
                text: &str,
            ) -> Result<(), String> {
                <$ty>::set_trailing_comment(self, tree, text)
            }
            fn clear_trailing_comment(&self, tree: &mut SyntaxTree) {
                <$ty>::clear_trailing_comment(self, tree)
            }
        }
    };
}

impl_entry!(BlockMappingEntry, key = |e: &BlockMappingEntry| e.key());
impl_entry!(FlowMappingEntry, key = |e: &FlowMappingEntry| e.key());
impl_entry!(BlockSequenceEntry, key = |_: &BlockSequenceEntry| None);
impl_entry!(FlowSequenceEntry, key = |_: &FlowSequenceEntry| None);

/// Shared interface for block- and flow-style mappings. Consumers that
/// don't care about style use this; style-specific accessors stay on the
/// concrete wrappers.
pub trait Mapping {
    /// Number of entries (including key-only entries).
    fn len(&self) -> usize;
    /// True when the mapping has no entries.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Iterate over `(key_node, value_node)` pairs in source order.
    /// Value is `None` only for key-only entries with no `:` slot; an
    /// empty value after a colon yields `Some` zero-width null scalar.
    fn pairs(&self) -> Box<dyn Iterator<Item = (Node, Option<Node>)> + '_>;
    /// Lookup by string key. Compares the decoded `Value::String` form of
    /// each key (plain keys also match because Core inference still
    /// produces strings for non-numeric/bool text like arbitrary keys).
    ///
    /// Returns `None` when the key is absent or the entry is key-only
    /// (no `:`). A present-but-empty value returns `Some` null scalar.
    fn get(&self, key: &str) -> Option<Node> {
        for (k, v) in self.pairs() {
            if let Node::Scalar(s) = &k
                && let Ok(decoded) = s.decoded()
                && decoded == key
            {
                return v;
            }
        }
        None
    }
    /// All keys, decoded as strings. Keys that can't be decoded are
    /// skipped (they'd need a different accessor to inspect).
    fn keys(&self) -> Box<dyn Iterator<Item = String> + '_> {
        Box::new(self.pairs().filter_map(|(k, _)| match k {
            Node::Scalar(s) => s.decoded().ok(),
            _ => None,
        }))
    }
    /// All values, in source order. Key-only entries are skipped.
    fn values(&self) -> Box<dyn Iterator<Item = Node> + '_> {
        Box::new(self.pairs().filter_map(|(_, v)| v))
    }
}

/// Shared interface for block- and flow-style sequences.
pub trait Sequence {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn iter(&self) -> Box<dyn Iterator<Item = Node> + '_>;
}

impl Mapping for BlockMapping {
    fn len(&self) -> usize {
        self.entries().count()
    }
    fn pairs(&self) -> Box<dyn Iterator<Item = (Node, Option<Node>)> + '_> {
        Box::new(
            self.entries()
                .filter_map(|e| e.key().map(|k| (k, e.value()))),
        )
    }
}

impl Mapping for FlowMapping {
    fn len(&self) -> usize {
        self.entries().count()
    }
    fn pairs(&self) -> Box<dyn Iterator<Item = (Node, Option<Node>)> + '_> {
        Box::new(
            self.entries()
                .filter_map(|e| e.key().map(|k| (k, e.value()))),
        )
    }
}

impl Sequence for BlockSequence {
    fn len(&self) -> usize {
        self.entries().count()
    }
    fn iter(&self) -> Box<dyn Iterator<Item = Node> + '_> {
        Box::new(self.entries().filter_map(|e| e.value()))
    }
}

impl Sequence for FlowSequence {
    fn len(&self) -> usize {
        self.entries().count()
    }
    fn iter(&self) -> Box<dyn Iterator<Item = Node> + '_> {
        Box::new(self.entries().filter_map(|e| e.value()))
    }
}

// Keep `Value` referenced so rustdoc links work and the import isn't
// flagged. It's used conceptually by consumers; no direct usage here.
#[allow(dead_code)]
fn _value_marker(_: Value) {}

#[cfg(test)]
mod tests {
    use crate::ast::nodes::AstNode;
    use crate::ast::{Node, Stream, Value};

    fn root(src: &str) -> Node {
        let tree = crate::parse(src).expect("parse");
        Stream::cast(tree.root().clone())
            .expect("stream")
            .documents()
            .next()
            .unwrap()
            .root_node()
            .unwrap()
    }

    #[test]
    fn block_mapping_len() {
        let n = root("a: 1\nb: 2\nc: 3\n");
        assert_eq!(n.as_mapping().unwrap().len(), 3);
    }

    #[test]
    fn block_mapping_get_returns_value_node() {
        let n = root("a: 1\nb: 2\n");
        let val = n.as_mapping().unwrap().get("b").expect("found b");
        match val {
            Node::Scalar(s) => assert_eq!(s.to_value().unwrap(), Value::Int(2)),
            _ => panic!(),
        }
    }

    #[test]
    fn block_mapping_get_missing() {
        let n = root("a: 1\n");
        assert!(n.as_mapping().unwrap().get("missing").is_none());
    }

    #[test]
    fn block_mapping_keys() {
        let n = root("a: 1\nb: 2\nc: 3\n");
        let keys: Vec<String> = n.as_mapping().unwrap().keys().collect();
        assert_eq!(keys, vec!["a", "b", "c"]);
    }

    #[test]
    fn block_mapping_get_nested() {
        let n = root("outer:\n  inner: 42\n");
        let inner_node = n.as_mapping().unwrap().get("outer").unwrap();
        let inner = inner_node.as_mapping().unwrap();
        assert_eq!(inner.len(), 1);
        match inner.get("inner").unwrap() {
            Node::Scalar(s) => assert_eq!(s.to_value().unwrap(), Value::Int(42)),
            _ => panic!(),
        }
    }

    #[test]
    fn flow_mapping_get() {
        let n = root("{a: 1, b: 2}\n");
        let val = n.as_mapping().unwrap().get("a").expect("a");
        match val {
            Node::Scalar(s) => assert_eq!(s.to_value().unwrap(), Value::Int(1)),
            _ => panic!(),
        }
    }

    #[test]
    fn flow_mapping_key_only_entry() {
        let n = root("{a}\n");
        let m = n.as_mapping().unwrap();
        assert_eq!(m.len(), 1);
        assert!(m.get("a").is_none());
    }

    #[test]
    fn block_sequence_len() {
        let n = root("- 1\n- 2\n- 3\n");
        assert_eq!(n.as_sequence().unwrap().len(), 3);
    }

    #[test]
    fn block_sequence_iter() {
        let n = root("- 1\n- 2\n- 3\n");
        let vals: Vec<Value> = n
            .as_sequence()
            .unwrap()
            .iter()
            .map(|n| match n {
                Node::Scalar(s) => s.to_value().unwrap(),
                _ => panic!(),
            })
            .collect();
        assert_eq!(vals, vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
    }

    #[test]
    fn flow_sequence_len() {
        let n = root("[a, b, c]\n");
        assert_eq!(n.as_sequence().unwrap().len(), 3);
    }

    #[test]
    fn flow_sequence_iter() {
        let n = root("[1, 2]\n");
        let vals: Vec<Value> = n
            .as_sequence()
            .unwrap()
            .iter()
            .map(|n| match n {
                Node::Scalar(s) => s.to_value().unwrap(),
                _ => panic!(),
            })
            .collect();
        assert_eq!(vals, vec![Value::Int(1), Value::Int(2)]);
    }

    #[test]
    fn scalar_is_not_mapping_or_sequence() {
        let n = root("42\n");
        assert!(n.as_mapping().is_none());
        assert!(n.as_sequence().is_none());
    }

    #[test]
    fn mapping_is_not_sequence() {
        let n = root("a: 1\n");
        assert!(n.as_sequence().is_none());
    }

    #[test]
    fn sequence_is_not_mapping() {
        let n = root("- 1\n");
        assert!(n.as_mapping().is_none());
    }
}
