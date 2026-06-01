use crate::SyntaxTree;
use crate::ast::comment as comment_impl;
use crate::ast::scalar::{Scalar, preceding_properties};
use crate::ast::trivia;
use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

pub trait AstNode: Sized {
    fn cast(node: SyntaxNode) -> Option<Self>;
    fn syntax(&self) -> &SyntaxNode;
    fn text_range(&self) -> rowan::TextRange {
        self.syntax().text_range()
    }
}

macro_rules! ast_node {
    ($name:ident, $kind:ident) => {
        #[repr(transparent)]
        #[derive(Debug, Clone)]
        pub struct $name(pub(crate) SyntaxNode);

        impl AstNode for $name {
            fn cast(n: SyntaxNode) -> Option<Self> {
                (n.kind() == SyntaxKind::$kind).then_some(Self(n))
            }
            fn syntax(&self) -> &SyntaxNode {
                &self.0
            }
        }
    };
}

ast_node!(Stream, STREAM);
ast_node!(Document, DOCUMENT);
ast_node!(Directives, DIRECTIVES);
ast_node!(BlockMapping, BLOCK_MAPPING);
ast_node!(BlockMappingEntry, BLOCK_MAPPING_ENTRY);
ast_node!(BlockSequence, BLOCK_SEQUENCE);
ast_node!(BlockSequenceEntry, BLOCK_SEQUENCE_ENTRY);
ast_node!(FlowMapping, FLOW_MAPPING);
ast_node!(FlowMappingEntry, FLOW_MAPPING_ENTRY);
ast_node!(FlowSequence, FLOW_SEQUENCE);
ast_node!(FlowSequenceEntry, FLOW_SEQUENCE_ENTRY);
ast_node!(AliasNode, ALIAS_NODE);
ast_node!(NodeProperties, NODE_PROPERTIES);

#[derive(Debug, Clone)]
pub enum Node {
    BlockMapping(BlockMapping),
    BlockSequence(BlockSequence),
    FlowMapping(FlowMapping),
    FlowSequence(FlowSequence),
    Scalar(Scalar),
    Alias(AliasNode),
}

impl Node {
    pub fn cast(n: SyntaxNode) -> Option<Self> {
        match n.kind() {
            SyntaxKind::BLOCK_MAPPING => BlockMapping::cast(n).map(Node::BlockMapping),
            SyntaxKind::BLOCK_SEQUENCE => BlockSequence::cast(n).map(Node::BlockSequence),
            SyntaxKind::FLOW_MAPPING => FlowMapping::cast(n).map(Node::FlowMapping),
            SyntaxKind::FLOW_SEQUENCE => FlowSequence::cast(n).map(Node::FlowSequence),
            SyntaxKind::SCALAR | SyntaxKind::NULL_SCALAR => Scalar::cast(n).map(Node::Scalar),
            SyntaxKind::ALIAS_NODE => AliasNode::cast(n).map(Node::Alias),
            _ => None,
        }
    }

    pub fn syntax(&self) -> &SyntaxNode {
        match self {
            Node::BlockMapping(x) => x.syntax(),
            Node::BlockSequence(x) => x.syntax(),
            Node::FlowMapping(x) => x.syntax(),
            Node::FlowSequence(x) => x.syntax(),
            Node::Scalar(x) => x.syntax(),
            Node::Alias(x) => x.syntax(),
        }
    }

    /// Node properties (anchor/tag) attached to this node. `Node::Alias`
    /// does not carry its own; aliases are themselves complete nodes and
    /// only the target node may have properties.
    pub fn properties(&self) -> Option<NodeProperties> {
        match self {
            Node::BlockMapping(x) => x.properties(),
            Node::BlockSequence(x) => x.properties(),
            Node::FlowMapping(x) => x.properties(),
            Node::FlowSequence(x) => x.properties(),
            Node::Scalar(x) => x.properties(),
            Node::Alias(_) => None,
        }
    }

    /// View this node as a [`Mapping`] if it's a block- or flow-style
    /// mapping. Returns `None` for scalars, sequences, and aliases.
    pub fn as_mapping(&self) -> Option<&dyn crate::ast::collections::Mapping> {
        match self {
            Node::BlockMapping(x) => Some(x),
            Node::FlowMapping(x) => Some(x),
            _ => None,
        }
    }

    /// View this node as a [`Sequence`] if it's a block- or flow-style
    /// sequence. Returns `None` for scalars, mappings, and aliases.
    pub fn as_sequence(&self) -> Option<&dyn crate::ast::collections::Sequence> {
        match self {
            Node::BlockSequence(x) => Some(x),
            Node::FlowSequence(x) => Some(x),
            _ => None,
        }
    }
}

fn first_child_token(n: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    n.children_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == kind)
}

fn first_node_child(n: &SyntaxNode) -> Option<Node> {
    n.children().find_map(Node::cast)
}

impl Stream {
    pub fn documents(&self) -> impl Iterator<Item = Document> + use<> {
        self.0.children().filter_map(Document::cast)
    }

    /// Number of documents in the stream.
    pub fn len(&self) -> usize {
        self.documents().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Stream-level leading comment: the contiguous comment block at
    /// the very start of the stream, *before* the first document's
    /// `---` marker. Returns empty when the stream starts with an
    /// implicit document (no `---`); in that case the top-of-file
    /// comment belongs to the document, not the stream.
    pub fn leading_comment(&self) -> String {
        let Some(first_doc) = self.documents().next() else {
            return String::new();
        };
        if first_doc.directives_end_token().is_none() {
            return String::new();
        }
        comment_impl::read_leading(first_doc.syntax())
    }

    /// Replace the stream-level leading comment with `text`. Errors if
    /// the first document has no `---` (there's no syntactic room for a
    /// stream-level comment — the whole top-of-file is document-leading).
    pub fn set_leading_comment(
        &self,
        tree: &mut SyntaxTree,
        text: Option<&str>,
    ) -> Result<(), String> {
        let Some(first_doc) = self.documents().next() else {
            return Err("stream has no documents".to_string());
        };
        if first_doc.directives_end_token().is_none() {
            return Err(
                "stream has no explicit `---` marker; the top-of-file comment belongs to \
                 the first document (use `d.set_comment(...)`)"
                    .to_string(),
            );
        }
        comment_impl::write_leading(tree, first_doc.syntax(), text);
        Ok(())
    }
}

impl Document {
    pub fn directives(&self) -> Option<Directives> {
        self.0.children().find_map(Directives::cast)
    }
    pub fn directives_end_token(&self) -> Option<SyntaxToken> {
        first_child_token(&self.0, SyntaxKind::DIRECTIVES_END)
    }
    pub fn document_end_token(&self) -> Option<SyntaxToken> {
        first_child_token(&self.0, SyntaxKind::DOCUMENT_END)
    }
    pub fn root_node(&self) -> Option<Node> {
        first_node_child(&self.0)
    }

    /// Leading comment for the document. With an explicit `---` marker
    /// we read the block inside `DOCUMENT` before the root node. Without
    /// one, the block lives in `STREAM` before the document.
    pub fn leading_comment(&self) -> String {
        if let Some(inner) = self.leading_comment_inner_anchor() {
            comment_impl::read_leading(&inner)
        } else {
            // Implicit document: the anchor is the DOCUMENT itself; its
            // leading trivia sits in the parent STREAM.
            comment_impl::read_leading(&self.0)
        }
    }

    pub fn set_leading_comment(&self, tree: &mut SyntaxTree, text: Option<&str>) {
        if let Some(inner) = self.leading_comment_inner_anchor() {
            comment_impl::write_leading(tree, &inner, text);
        } else {
            comment_impl::write_leading(tree, &self.0, text);
        }
    }

    /// For documents with an explicit `---`, return the first node-child
    /// inside the document that comments attach in front of. None when
    /// implicit (no `---`).
    fn leading_comment_inner_anchor(&self) -> Option<SyntaxNode> {
        self.directives_end_token()?;
        // Anchor is the first child AFTER `---` that is either the root
        // node or something that would be the target. We use the root
        // node when present; otherwise fall through to None (nothing to
        // anchor on).
        self.0.children().find(|c| {
            matches!(
                c.kind(),
                SyntaxKind::BLOCK_MAPPING
                    | SyntaxKind::BLOCK_SEQUENCE
                    | SyntaxKind::FLOW_MAPPING
                    | SyntaxKind::FLOW_SEQUENCE
                    | SyntaxKind::SCALAR
                    | SyntaxKind::NULL_SCALAR
                    | SyntaxKind::ALIAS_NODE
            )
        })
    }
}

impl Directives {
    pub fn directives(&self) -> impl Iterator<Item = SyntaxToken> + use<> {
        self.0
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .filter(|t| t.kind() == SyntaxKind::DIRECTIVE)
    }
}

impl BlockMapping {
    pub fn entries(&self) -> impl Iterator<Item = BlockMappingEntry> + use<> {
        self.0.children().filter_map(BlockMappingEntry::cast)
    }
    pub fn properties(&self) -> Option<NodeProperties> {
        preceding_properties(&self.0)
    }
}

impl BlockMappingEntry {
    pub fn question_token(&self) -> Option<SyntaxToken> {
        first_child_token(&self.0, SyntaxKind::QUESTION)
    }
    pub fn key(&self) -> Option<Node> {
        self.0.children().filter_map(Node::cast).next()
    }
    pub fn colon_token(&self) -> Option<SyntaxToken> {
        first_child_token(&self.0, SyntaxKind::COLON)
    }
    pub fn value(&self) -> Option<Node> {
        self.0.children().filter_map(Node::cast).nth(1)
    }
    pub fn leading_trivia(&self) -> std::vec::IntoIter<SyntaxToken> {
        trivia::leading_trivia(&self.0)
    }
    pub fn trailing_trivia(&self) -> std::vec::IntoIter<SyntaxToken> {
        trivia::trailing_trivia(&self.0)
    }

    /// Normalized payload of the contiguous leading comment block
    /// attached to this entry. See `ast/comment.rs` for rules.
    pub fn leading_comment(&self) -> String {
        comment_impl::read_leading(&self.0)
    }

    /// Replace the leading comment block with `text`. Pass `None` to
    /// clear. Multiline strings emit one `#`-prefixed line per
    /// `\n`-separated segment, indented to match this entry's column.
    /// `Err` only when the entry has no room for a comment line above
    /// (never for a block entry, which always starts a fresh line).
    pub fn set_leading_comment(
        &self,
        tree: &mut SyntaxTree,
        text: Option<&str>,
    ) -> Result<(), String> {
        comment_impl::write_leading_guarded(tree, &self.0, text)
    }

    /// Normalized payload of the same-line trailing comment on this
    /// entry. Empty when absent.
    pub fn trailing_comment(&self) -> String {
        comment_impl::read_trailing(&self.0)
    }

    /// Set the same-line trailing comment. Returns `Err` when `text`
    /// contains a newline.
    pub fn set_trailing_comment(&self, tree: &mut SyntaxTree, text: &str) -> Result<(), String> {
        comment_impl::write_trailing(tree, &self.0, text)
    }

    pub fn clear_trailing_comment(&self, tree: &mut SyntaxTree) {
        comment_impl::clear_trailing(tree, &self.0);
    }
}

impl BlockSequence {
    pub fn entries(&self) -> impl Iterator<Item = BlockSequenceEntry> + use<> {
        self.0.children().filter_map(BlockSequenceEntry::cast)
    }
    pub fn properties(&self) -> Option<NodeProperties> {
        preceding_properties(&self.0)
    }
}

impl BlockSequenceEntry {
    pub fn dash_token(&self) -> Option<SyntaxToken> {
        first_child_token(&self.0, SyntaxKind::DASH)
    }
    pub fn value(&self) -> Option<Node> {
        first_node_child(&self.0)
    }
    pub fn leading_trivia(&self) -> std::vec::IntoIter<SyntaxToken> {
        trivia::leading_trivia(&self.0)
    }
    pub fn trailing_trivia(&self) -> std::vec::IntoIter<SyntaxToken> {
        trivia::trailing_trivia(&self.0)
    }

    /// Leading comment block for this sequence entry. When the entry is
    /// the first element of a sequence that starts on a line below a
    /// mapping key (e.g. `items:\n  # about a\n  - a`), the parser
    /// attaches the `# about a` comment to the outer `BLOCK_MAPPING_ENTRY`
    /// rather than to this sequence or its first entry. We check that
    /// outer location when the entry's own slot is empty.
    pub fn leading_comment(&self) -> String {
        let direct = comment_impl::read_leading(&self.0);
        if !direct.is_empty() {
            return direct;
        }
        if let Some(outer) = self.first_element_outer_anchor() {
            return comment_impl::read_leading(&outer);
        }
        String::new()
    }

    /// Set the leading comment. When this entry is the first element of
    /// its sequence and the outer mapping entry already hosts a comment
    /// meant for this element, replace it there. Otherwise attach a
    /// fresh comment at this entry's own position.
    pub fn set_leading_comment(
        &self,
        tree: &mut SyntaxTree,
        text: Option<&str>,
    ) -> Result<(), String> {
        if comment_impl::read_leading(&self.0).is_empty()
            && let Some(outer) = self.first_element_outer_anchor()
            && !comment_impl::read_leading(&outer).is_empty()
        {
            return comment_impl::write_leading_guarded(tree, &outer, text);
        }
        comment_impl::write_leading_guarded(tree, &self.0, text)
    }

    pub fn trailing_comment(&self) -> String {
        comment_impl::read_trailing(&self.0)
    }

    pub fn set_trailing_comment(&self, tree: &mut SyntaxTree, text: &str) -> Result<(), String> {
        comment_impl::write_trailing(tree, &self.0, text)
    }

    pub fn clear_trailing_comment(&self, tree: &mut SyntaxTree) {
        comment_impl::clear_trailing(tree, &self.0);
    }

    /// If this entry is the first element of its parent `BLOCK_SEQUENCE`,
    /// and that sequence is the value of some `BLOCK_MAPPING_ENTRY`,
    /// returns the sequence itself — the leading trivia of a first-
    /// nested-element lives just before the sequence inside the outer
    /// mapping entry, so `read_leading(seq)` finds it.
    fn first_element_outer_anchor(&self) -> Option<SyntaxNode> {
        let seq = self.0.parent()?;
        if seq.kind() != SyntaxKind::BLOCK_SEQUENCE {
            return None;
        }
        let mut first_entry: Option<SyntaxNode> = None;
        for c in seq.children() {
            if c.kind() == SyntaxKind::BLOCK_SEQUENCE_ENTRY {
                first_entry = Some(c);
                break;
            }
        }
        if first_entry.as_ref() != Some(&self.0) {
            return None;
        }
        let outer = seq.parent()?;
        if outer.kind() == SyntaxKind::BLOCK_MAPPING_ENTRY {
            Some(seq)
        } else {
            None
        }
    }
}

impl FlowMapping {
    pub fn l_brace(&self) -> Option<SyntaxToken> {
        first_child_token(&self.0, SyntaxKind::L_BRACE)
    }
    pub fn r_brace(&self) -> Option<SyntaxToken> {
        first_child_token(&self.0, SyntaxKind::R_BRACE)
    }
    pub fn entries(&self) -> impl Iterator<Item = FlowMappingEntry> + use<> {
        self.0.children().filter_map(FlowMappingEntry::cast)
    }
    pub fn properties(&self) -> Option<NodeProperties> {
        preceding_properties(&self.0)
    }
}

impl FlowMappingEntry {
    pub fn question_token(&self) -> Option<SyntaxToken> {
        first_child_token(&self.0, SyntaxKind::QUESTION)
    }
    pub fn key(&self) -> Option<Node> {
        self.0.children().filter_map(Node::cast).next()
    }
    pub fn colon_token(&self) -> Option<SyntaxToken> {
        first_child_token(&self.0, SyntaxKind::COLON)
    }
    pub fn value(&self) -> Option<Node> {
        self.0.children().filter_map(Node::cast).nth(1)
    }

    pub fn leading_comment(&self) -> String {
        comment_impl::read_leading(&self.0)
    }
    pub fn set_leading_comment(
        &self,
        tree: &mut SyntaxTree,
        text: Option<&str>,
    ) -> Result<(), String> {
        comment_impl::write_leading_guarded(tree, &self.0, text)
    }
    pub fn trailing_comment(&self) -> String {
        comment_impl::read_trailing(&self.0)
    }
    pub fn set_trailing_comment(&self, tree: &mut SyntaxTree, text: &str) -> Result<(), String> {
        comment_impl::write_trailing(tree, &self.0, text)
    }
    pub fn clear_trailing_comment(&self, tree: &mut SyntaxTree) {
        comment_impl::clear_trailing(tree, &self.0);
    }
}

impl FlowSequence {
    pub fn l_bracket(&self) -> Option<SyntaxToken> {
        first_child_token(&self.0, SyntaxKind::L_BRACKET)
    }
    pub fn r_bracket(&self) -> Option<SyntaxToken> {
        first_child_token(&self.0, SyntaxKind::R_BRACKET)
    }
    pub fn entries(&self) -> impl Iterator<Item = FlowSequenceEntry> + use<> {
        self.0.children().filter_map(FlowSequenceEntry::cast)
    }
    pub fn properties(&self) -> Option<NodeProperties> {
        preceding_properties(&self.0)
    }
}

impl FlowSequenceEntry {
    pub fn value(&self) -> Option<Node> {
        first_node_child(&self.0)
    }

    pub fn leading_comment(&self) -> String {
        comment_impl::read_leading(&self.0)
    }
    pub fn set_leading_comment(
        &self,
        tree: &mut SyntaxTree,
        text: Option<&str>,
    ) -> Result<(), String> {
        comment_impl::write_leading_guarded(tree, &self.0, text)
    }
    pub fn trailing_comment(&self) -> String {
        comment_impl::read_trailing(&self.0)
    }
    pub fn set_trailing_comment(&self, tree: &mut SyntaxTree, text: &str) -> Result<(), String> {
        comment_impl::write_trailing(tree, &self.0, text)
    }
    pub fn clear_trailing_comment(&self, tree: &mut SyntaxTree) {
        comment_impl::clear_trailing(tree, &self.0);
    }
}

impl AliasNode {
    pub fn alias_token(&self) -> Option<SyntaxToken> {
        first_child_token(&self.0, SyntaxKind::ALIAS)
    }
    pub fn properties(&self) -> Option<NodeProperties> {
        preceding_properties(&self.0)
    }
    /// The alias's referenced name (text after the leading `*`).
    pub fn name(&self) -> Option<String> {
        self.alias_token()
            .map(|t| t.text().trim_start_matches('*').to_string())
    }
}

impl NodeProperties {
    pub fn anchor(&self) -> Option<SyntaxToken> {
        first_child_token(&self.0, SyntaxKind::ANCHOR)
    }
    pub fn tag(&self) -> Option<SyntaxToken> {
        first_child_token(&self.0, SyntaxKind::TAG)
    }
    /// Anchor + tag tokens in source order (interleaved trivia skipped).
    pub fn properties_in_order(&self) -> impl Iterator<Item = SyntaxToken> + use<> {
        self.0
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .filter(|t| matches!(t.kind(), SyntaxKind::ANCHOR | SyntaxKind::TAG))
    }

    /// Anchor name (text after the leading `&`).
    pub fn anchor_name(&self) -> Option<String> {
        self.anchor()
            .map(|t| t.text().trim_start_matches('&').to_string())
    }

    /// Parsed tag. Returns `None` when no tag is attached.
    pub fn tag_value(&self) -> Option<Tag> {
        self.tag().map(|t| Tag::parse(t.text()))
    }
}

/// A parsed tag. The lexer preserves raw text; this breaks it into its
/// YAML 1.2 variants. `Unknown` is used for malformed tag text that the
/// lexer admitted (e.g., `!!!` sequences).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tag {
    /// `!` — the non-specific tag.
    NonSpecific,
    /// `!<URI>` — verbatim tag with the URI between `<` and `>`.
    Verbatim(String),
    /// `!!name` — secondary-handle shorthand (the `!!` prefix).
    Secondary(String),
    /// `!handle!suffix` — named-handle shorthand.
    Handle { handle: String, suffix: String },
    /// `!suffix` — primary-handle shorthand (a.k.a. local tag).
    Primary(String),
    /// Syntactically present but not matching any recognized form.
    Unknown(String),
}

impl Tag {
    fn parse(raw: &str) -> Self {
        // Non-specific: bare `!`.
        if raw == "!" {
            return Tag::NonSpecific;
        }
        // Verbatim: `!<...>`.
        if let Some(rest) = raw.strip_prefix("!<") {
            if let Some(uri) = rest.strip_suffix('>') {
                return Tag::Verbatim(uri.to_string());
            }
            return Tag::Unknown(raw.to_string());
        }
        // Secondary: `!!...`.
        if let Some(rest) = raw.strip_prefix("!!") {
            return Tag::Secondary(rest.to_string());
        }
        // Named handle: `!handle!suffix`.
        if let Some(rest) = raw.strip_prefix('!') {
            if let Some(bang) = rest.find('!') {
                let (handle, suffix) = rest.split_at(bang);
                return Tag::Handle {
                    handle: handle.to_string(),
                    suffix: suffix[1..].to_string(),
                };
            }
            // Primary: `!suffix`.
            return Tag::Primary(rest.to_string());
        }
        Tag::Unknown(raw.to_string())
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::{AstNode, Node, Stream, Tag};

    fn parse(src: &'static str) -> Stream {
        let tree = crate::parse(src).expect("parse");
        Stream::cast(tree.root().clone()).expect("stream")
    }

    fn first_doc_root(src: &'static str) -> Node {
        parse(src)
            .documents()
            .next()
            .expect("doc")
            .root_node()
            .expect("root")
    }

    #[test]
    fn anchor_name_on_scalar() {
        let node = first_doc_root("&abc hello\n");
        let props = node.properties().expect("props");
        assert_eq!(props.anchor_name().as_deref(), Some("abc"));
    }

    #[test]
    fn anchor_name_empty_when_absent() {
        let node = first_doc_root("hello\n");
        assert!(node.properties().is_none());
    }

    #[test]
    fn alias_name_on_scalar() {
        let node = first_doc_root("*abc\n");
        match node {
            Node::Alias(a) => assert_eq!(a.name().as_deref(), Some("abc")),
            _ => panic!("expected alias"),
        }
    }

    #[test]
    fn alias_in_mapping_value() {
        // key: *abc
        let stream = parse("key: *abc\n");
        let doc = stream.documents().next().unwrap();
        let map = match doc.root_node().unwrap() {
            Node::BlockMapping(m) => m,
            _ => panic!(),
        };
        let entry = map.entries().next().unwrap();
        match entry.value().unwrap() {
            Node::Alias(a) => assert_eq!(a.name().as_deref(), Some("abc")),
            _ => panic!(),
        }
    }

    #[test]
    fn tag_secondary_shorthand() {
        let node = first_doc_root("!!str 1\n");
        let props = node.properties().expect("props");
        assert_eq!(props.tag_value().unwrap(), Tag::Secondary("str".into()));
    }

    #[test]
    fn tag_primary_local() {
        let node = first_doc_root("!local 1\n");
        let props = node.properties().expect("props");
        assert_eq!(props.tag_value().unwrap(), Tag::Primary("local".into()));
    }

    #[test]
    fn tag_verbatim() {
        let node = first_doc_root("!<tag:example.com,2024:foo> 1\n");
        let props = node.properties().expect("props");
        assert_eq!(
            props.tag_value().unwrap(),
            Tag::Verbatim("tag:example.com,2024:foo".into())
        );
    }

    #[test]
    fn tag_named_handle() {
        let node = first_doc_root("!e!foo 1\n");
        let props = node.properties().expect("props");
        assert_eq!(
            props.tag_value().unwrap(),
            Tag::Handle {
                handle: "e".into(),
                suffix: "foo".into()
            }
        );
    }

    #[test]
    fn tag_non_specific() {
        let node = first_doc_root("! 1\n");
        let props = node.properties().expect("props");
        assert_eq!(props.tag_value().unwrap(), Tag::NonSpecific);
    }

    #[test]
    fn anchor_and_tag_together() {
        let node = first_doc_root("!!str &a hello\n");
        let props = node.properties().expect("props");
        assert_eq!(props.anchor_name().as_deref(), Some("a"));
        assert_eq!(props.tag_value().unwrap(), Tag::Secondary("str".into()));
    }
}
