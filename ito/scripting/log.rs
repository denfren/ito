//! `log` module: structured logging into the Rust `tracing` log.
//!
//! - `log.error(...)` / `log.warn(...)` / `log.info(...)` / `log.debug(...)`
//!   / `log.trace(...)` — log the arguments (joined like `print`) at the
//!   matching `tracing` level, under the `ito::lua::log` target.
//!
//! Distinct from the top-level `print` global (which models script
//! output, not logging): every `log.*` call always goes through
//! `tracing`, regardless of `--quiet`/`--diff`/`--list-changed`.

use mlua::Lua;

/// `tostring`-style rendering for `log.*` arguments (matches `print`).
fn display_value(v: &mlua::Value) -> String {
    v.to_string().unwrap_or_else(|e| format!("<error: {e}>"))
}

/// Register the `log` module on `lua`.
pub fn register(lua: &Lua) -> mlua::Result<()> {
    let module = lua.create_table()?;

    macro_rules! level_fn {
        ($name:literal, $macro:ident) => {
            module.set(
                $name,
                lua.create_function(move |_, args: mlua::Variadic<mlua::Value>| {
                    let parts: Vec<String> = args.iter().map(display_value).collect();
                    tracing::$macro!(target: "ito::lua::log", "{}", parts.join("\t"));
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
            lua.load(format!(r#"log.{level}("hello", 1, true)"#))
                .exec()
                .unwrap_or_else(|e| panic!("log.{level} failed: {e}"));
        }
    }
}
