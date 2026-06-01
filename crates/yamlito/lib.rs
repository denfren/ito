//! YAML parser and lossless concrete-syntax-tree editor.
//!
//! This crate isolates all YAML-specific code (lexer, parser, syntax
//! tree, typed AST, surgical edits, fixers, and the Rhai cursor glue).

pub mod ast;
pub mod builder;
pub mod debug;
pub mod edit;
pub mod error;
pub mod fixers;
pub mod lexer;
pub(crate) mod parser;
pub mod scripting;
pub mod syntax;

pub use builder::Yaml;
pub use error::ParseError;
pub use parser::{parse, parse_recover};
pub use syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken, SyntaxTree, YamlLang};
