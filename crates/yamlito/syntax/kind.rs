use SyntaxKind::*;
use num_enum::TryFromPrimitive;

#[allow(non_camel_case_types)]
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, TryFromPrimitive)]
pub enum SyntaxKind {
    WHITESPACE,
    NEWLINE,
    COMMENT,
    BOM,

    PLAIN_SCALAR,
    SINGLE_QUOTED_SCALAR,
    DOUBLE_QUOTED_SCALAR,
    LITERAL_SCALAR,
    FOLDED_SCALAR,

    COLON,
    DASH,
    QUESTION,
    COMMA,
    L_BRACKET,
    R_BRACKET,
    L_BRACE,
    R_BRACE,

    ANCHOR,
    ALIAS,
    TAG,

    DIRECTIVE,
    DIRECTIVES_END,
    DOCUMENT_END,

    ERROR,

    STREAM,
    DOCUMENT,
    DIRECTIVES,
    BLOCK_MAPPING,
    BLOCK_MAPPING_ENTRY,
    BLOCK_SEQUENCE,
    BLOCK_SEQUENCE_ENTRY,
    FLOW_MAPPING,
    FLOW_MAPPING_ENTRY,
    FLOW_SEQUENCE,
    FLOW_SEQUENCE_ENTRY,
    SCALAR,
    /// Zero-width node standing in for an implicit null value: an empty
    /// mapping value (`key:` at EOL) or empty sequence entry (`-` at
    /// EOF). Always emitted so `value()` is uniformly `Some`. Carries no
    /// tokens, so it contributes nothing to the emitted text.
    NULL_SCALAR,
    ALIAS_NODE,
    NODE_PROPERTIES,

    #[doc(hidden)]
    __LAST,
}

impl SyntaxKind {
    pub fn is_trivia(self) -> bool {
        matches!(self, WHITESPACE | NEWLINE | COMMENT)
    }

    pub fn is_token(self) -> bool {
        (self as u16) < (STREAM as u16)
    }

    pub fn is_node(self) -> bool {
        !self.is_token() && self != __LAST
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum YamlLang {}

impl rowan::Language for YamlLang {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> SyntaxKind {
        SyntaxKind::try_from(raw.0).expect("invalid SyntaxKind raw value")
    }

    fn kind_to_raw(kind: SyntaxKind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}

pub type SyntaxNode = rowan::SyntaxNode<YamlLang>;
pub type SyntaxToken = rowan::SyntaxToken<YamlLang>;
pub type SyntaxElement = rowan::SyntaxElement<YamlLang>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_vs_node_split() {
        assert!(WHITESPACE.is_token());
        assert!(PLAIN_SCALAR.is_token());
        assert!(ERROR.is_token());
        assert!(STREAM.is_node());
        assert!(BLOCK_MAPPING.is_node());
        assert!(!__LAST.is_node());
    }

    #[test]
    fn trivia_set() {
        assert!(WHITESPACE.is_trivia());
        assert!(NEWLINE.is_trivia());
        assert!(COMMENT.is_trivia());
        assert!(!BOM.is_trivia());
        assert!(!COLON.is_trivia());
    }

    #[test]
    fn round_trip_raw_kind() {
        use rowan::Language;
        for kind in [WHITESPACE, COLON, DIRECTIVE, STREAM, SCALAR] {
            let raw = YamlLang::kind_to_raw(kind);
            assert_eq!(YamlLang::kind_from_raw(raw), kind);
        }
    }
}
