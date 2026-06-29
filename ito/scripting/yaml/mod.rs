//! `yaml` module: convert between YAML text and native Lua values.
//!
//! - `yaml.parse(text)` parses via the `yamlito` crate, decodes the
//!   (single) document into a `yamlito::Yaml` value using the YAML 1.2
//!   Core schema, then converts to a Lua value (tables, strings,
//!   numbers, bools, nil), so scripts can inspect YAML content with
//!   ordinary Lua indexing and iteration. Multi-document streams and
//!   anchor aliases are rejected.
//! - `yaml.to_string(value)` serializes a Lua value back to canonical
//!   block YAML text (2-space indented, always ending in a newline).
//! - `yaml.parse_multi(text)` parses a multi-document stream (`---`
//!   separators), returning an array with one value per document.
//! - `yaml.to_string_multi(values)` serializes an array of values to a
//!   multi-document stream, separating documents with `---`.
//! - `yaml.edit(text)` parses into a format-preserving, editable handle
//!   (a `YamlCursor` over the stream root). Unlike `parse`/`to_string`
//!   (lossy), edits are surgical — layout and comments survive. See the
//!   `edit` submodule.

mod comment;
mod edit;

use mlua::{Lua, LuaSerdeExt, Value};
use yamlito::Yaml;

fn parse_err(e: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(format!("yaml parse error: {e}"))
}

/// Register the `yaml` module on `lua`.
pub fn register(lua: &Lua) -> mlua::Result<()> {
    let module = lua.create_table()?;

    module.set(
        "parse",
        lua.create_function(|lua, text: String| {
            let tree = yamlito::parse(&text).map_err(parse_err)?;
            let value = Yaml::from_tree(&tree).map_err(parse_err)?;
            lua.to_value(&value)
        })?,
    )?;

    module.set(
        "to_string",
        lua.create_function(|lua, value: Value| {
            let v: Yaml = lua.from_value(value)?;
            Ok(v.to_yaml_string())
        })?,
    )?;

    module.set(
        "parse_multi",
        lua.create_function(|lua, text: String| {
            let tree = yamlito::parse(&text).map_err(parse_err)?;
            let docs = Yaml::from_stream(&tree).map_err(parse_err)?;
            lua.to_value(&docs)
        })?,
    )?;

    module.set(
        "to_string_multi",
        lua.create_function(|lua, values: Value| {
            let docs: Vec<Yaml> = lua.from_value(values)?;
            Ok(Yaml::to_multi_string(&docs))
        })?,
    )?;

    edit::register(lua, &module)?;

    lua.globals().set("yaml", module)?;
    Ok(())
}
