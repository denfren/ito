# yamlito

A standalone YAML parser and lossless concrete-syntax-tree (CST) editor.
See `DIALECT.md` (crate root) for yamlito's YAML 1.2 dialect notes:
spec-conformant cases worth calling out and known limitations.

## Layout

Modules (paths use `crate::*`):

- `lexer` — tokenizer.
- `parser` — produces the rowan syntax tree (`pub(crate)`; entry points
  re-exported as `parse`/`parse_recover`).
- `syntax` — rowan tree types (`SyntaxNode`, `SyntaxKind`, `SyntaxTree`,
  `YamlLang`, …).
- `ast` — typed AST over the syntax tree.
- `edit` — surgical CST edits.
- `fixers` — automated fix-ups.
- `quoting` — scalar quoting decisions (owns the only double-quote
  emitter in the crate).
- `builder` — the serde value model (`Yaml`, plus `Yaml::from_tree` for
  text→value).
- `error` — `ParseError`.
- `debug` — debug rendering helpers.

`ito` depends on this crate for its `yaml` module via `builder::Yaml`;
the rest — surgical edits and fixers — compiles but is not consumed by
any binary yet.

## Tests

Tests live in `tests/`: `parser.rs` (CST/AST insta snapshot tests over
`tests/yaml/**/*.yml`, snapshots in `tests/snapshots/`), plus
round-trip and value-sample tests. Run via `cargo test -p yamlito`.
