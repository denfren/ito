//! `yaml` module: convert between YAML text and native Rhai values.
//!
//! - `yaml::parse(text)` parses via the `yamlito` crate, decodes the
//!   (single) document into a `yamlito::Yaml` value using the YAML 1.2
//!   Core schema, then converts to a Rhai `Dynamic` (maps, arrays,
//!   strings, ints/floats, bools, ()), so scripts can inspect YAML
//!   content with ordinary Rhai indexing and iteration. Multi-document
//!   streams and anchor aliases are rejected.
//! - `yaml::to_string(value)` serializes a Rhai value back to canonical
//!   block YAML text (2-space indented, always ending in a newline).
//! - `yaml::parse_multi(text)` parses a multi-document stream (`---`
//!   separators), returning an array with one value per document.
//! - `yaml::to_string_multi(values)` serializes an array of values to a
//!   multi-document stream, separating documents with `---`.

use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Module};
use yamlito::Yaml;

fn parse_err(e: impl std::fmt::Display) -> Box<EvalAltResult> {
    format!("yaml parse error: {e}").into()
}

/// Register the `yaml` module on `engine`.
pub fn register(engine: &mut Engine) {
    let mut module = Module::new();
    module.set_native_fn(
        "parse",
        |text: ImmutableString| -> Result<Dynamic, Box<EvalAltResult>> {
            let tree = yamlito::parse(&text).map_err(parse_err)?;
            let value = Yaml::from_tree(&tree).map_err(parse_err)?;
            rhai::serde::to_dynamic(value)
        },
    );
    module.set_native_fn(
        "to_string",
        |value: Dynamic| -> Result<String, Box<EvalAltResult>> {
            let v: Yaml = rhai::serde::from_dynamic(&value)?;
            Ok(v.to_yaml_string())
        },
    );
    module.set_native_fn(
        "parse_multi",
        |text: ImmutableString| -> Result<Dynamic, Box<EvalAltResult>> {
            let tree = yamlito::parse(&text).map_err(parse_err)?;
            let docs = Yaml::from_stream(&tree).map_err(parse_err)?;
            rhai::serde::to_dynamic(docs)
        },
    );
    module.set_native_fn(
        "to_string_multi",
        |values: Dynamic| -> Result<String, Box<EvalAltResult>> {
            let docs: Vec<Yaml> = rhai::serde::from_dynamic(&values)?;
            Ok(Yaml::to_multi_string(&docs))
        },
    );
    engine.register_static_module("yaml", module.into());
}
