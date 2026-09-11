//! `log` module: logging into the Rust `tracing` log.
//!
//! - `log.error(...)` / `log.warn(...)` / `log.info(...)` / `log.debug(...)`
//!   / `log.trace(...)` — log the arguments (joined with tabs; plain tables
//!   as JSON, unlike `print`) at the matching `tracing` level, under the `ito::lua::log`
//!   target. A failing `__tostring` raises.
//!
//! Distinct from the top-level `print` global (which models script
//! output, not logging): every `log.*` call always goes through
//! `tracing`, regardless of `--quiet`. Logging
//! never changes the exit code, `log.error` included. Control characters
//! (ANSI escapes) in a message are escaped by the subscriber, not stripped.

use mlua::{Lua, LuaSerdeExt, Value};

/// `tostring`-style rendering for log messages. A plain table (no
/// `__tostring`) renders as JSON; a failing `__tostring` raises.
fn display_value(lua: &Lua, v: &Value) -> mlua::Result<String> {
    if let Value::Table(t) = v
        && t.metatable()
            .is_none_or(|m| !m.contains_key("__tostring").unwrap_or(false))
        && let Ok(j) = lua.from_value::<serde_json::Value>(v.clone())
    {
        return Ok(j.to_string());
    }
    Ok(v.to_string()?)
}

/// Join arguments with tabs.
fn join_args(lua: &Lua, args: &[Value]) -> mlua::Result<String> {
    let parts: mlua::Result<Vec<String>> = args.iter().map(|v| display_value(lua, v)).collect();
    Ok(parts?.join("\t"))
}

/// Register the `log` module on `lua`.
pub fn register(lua: &Lua) -> mlua::Result<()> {
    let module = lua.create_table()?;

    macro_rules! level_fn {
        ($name:literal, $macro:ident) => {
            module.set(
                $name,
                lua.create_function(move |lua, args: mlua::Variadic<mlua::Value>| {
                    let text = join_args(lua, &args)?;
                    tracing::$macro!(target: "ito::lua::log", "{}", text);
                    Ok(())
                })?,
            )?;
        };
    }
    level_fn!("error", error);
    level_fn!("warn", warn);
    level_fn!("info", info);
    level_fn!("debug", debug);
    level_fn!("trace", trace);

    lua.globals().set("log", module)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua() -> Lua {
        let lua = Lua::new();
        register(&lua).expect("register");
        lua
    }

    #[test]
    fn all_levels_callable() {
        let lua = lua();
        for level in ["error", "warn", "info", "debug", "trace"] {
            lua.load(format!(r#"log.{level}("hello", 1, true, {{a = 1}})"#))
                .exec()
                .unwrap_or_else(|e| panic!("log.{level} failed: {e}"));
        }
    }

    #[test]
    fn failing_tostring_raises() {
        let lua = lua();
        assert!(
            lua.load("log.info(setmetatable({}, {__tostring = function() error('x') end}))")
                .exec()
                .is_err()
        );
    }
}
