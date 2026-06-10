//! YAML parser and lossless concrete-syntax-tree editor.
//!
//! See `DIALECT.md` (crate root) for yamlito's YAML 1.2 dialect notes:
//! spec-conformant cases worth calling out and known limitations.

pub mod ast;
pub mod builder;
pub mod debug;
pub mod edit;
pub mod error;
pub mod fixers;
pub mod lexer;
pub(crate) mod parser;
pub mod quoting;
pub mod syntax;

pub use builder::Yaml;
pub use error::ParseError;
pub use parser::{parse, parse_recover};
pub use syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken, SyntaxTree, YamlLang};
