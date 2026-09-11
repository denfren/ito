//! `yaml` module: convert between YAML text and native Lua values.
//!
//! - `yaml.parse(text, name?)` parses via the `yamlito` crate, decodes
//!   the (single) document into a `yamlito::Yaml` value using the YAML
//!   1.2 Core schema, then converts to a Lua value (tables, strings,
//!   numbers, bools, nil), so scripts can inspect YAML content with
//!   ordinary Lua indexing and iteration. Multi-document streams and
//!   anchor aliases are rejected. `name` (a file name, say) is added to
//!   parse errors.
//! - `yaml.to_string(value)` serializes a Lua value back to canonical
//!   block YAML text (2-space indented, always ending in a newline).
//! - `yaml.parse_multi(text, name?)` parses a multi-document stream
//!   (`---` separators), returning an array with one value per document.
//!   `name` is added to parse errors.
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

fn parse_err(name: &Option<String>, e: impl std::fmt::Display) -> mlua::Error {
    match name {
        Some(n) => mlua::Error::runtime(format!("yaml parse error in {n}: {e}")),
        None => mlua::Error::runtime(format!("yaml parse error: {e}")),
    }
}

/// Register the `yaml` module on `lua`.
pub fn register(lua: &Lua) -> mlua::Result<()> {
    let module = lua.create_table()?;

    module.set(
        "parse",
        lua.create_function(|lua, (text, name): (String, Option<String>)| {
            let tree = yamlito::parse(&text).map_err(|e| parse_err(&name, e))?;
            let value = Yaml::from_tree(&tree).map_err(|e| parse_err(&name, e))?;
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
        lua.create_function(|lua, (text, name): (String, Option<String>)| {
            let tree = yamlito::parse(&text).map_err(|e| parse_err(&name, e))?;
            let docs = Yaml::from_stream(&tree).map_err(|e| parse_err(&name, e))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn lua() -> Lua {
        let lua = Lua::new();
        register(&lua).unwrap();
        lua
    }

    fn err(lua: &Lua, src: &str) -> String {
        lua.load(src).exec().unwrap_err().to_string()
    }

    #[test]
    fn parse_uses_optional_name_in_errors() {
        let lua = lua();
        let err = err(&lua, r#"yaml.parse("a: [", "settings.yml")"#);
        assert!(err.contains("yaml parse error in settings.yml:"), "{err}");
    }

    #[test]
    fn parse_multi_uses_optional_name_in_errors() {
        let lua = lua();
        let err = err(&lua, r#"yaml.parse_multi("a: [", "settings.yml")"#);
        assert!(err.contains("yaml parse error in settings.yml:"), "{err}");
    }

    #[test]
    fn parse_without_a_name_keeps_the_old_message() {
        let lua = lua();
        let err = err(&lua, r#"yaml.parse("a: [")"#);
        assert!(err.contains("yaml parse error:"), "{err}");
        assert!(!err.contains("yaml parse error in"), "{err}");
    }

    #[test]
    fn a_name_does_not_affect_a_successful_parse() {
        let lua = lua();
        let value: String = lua
            .load(r#"return yaml.parse("a: 1", "settings.yml").a"#)
            .eval()
            .unwrap();
        assert_eq!(value, "1");
        let count: i64 = lua
            .load(r#"return #yaml.parse_multi("a: 1\n---\nb: 2", "stream.yml")"#)
            .eval()
            .unwrap();
        assert_eq!(count, 2);
    }
}
