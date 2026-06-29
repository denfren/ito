//! `io` module: raw stream writes, host-only.
//!
//! - `io.write(...)` / `io.writeln(...)` — write to stdout, joined
//!   `tostring`-style like `print` (`writeln` adds the trailing newline).
//! - `io.ewrite(...)` / `io.ewriteln(...)` — the same, to stderr.
//!
//! Distinct from `log.*`: these are raw stream writes, not `tracing` records
//! — no level, no target, no `RUST_LOG`/`ITO_LOG` filtering, and no ANSI
//! stripping, so `io.ewriteln(("x"):red())` reaches the terminal styled.
//! `ito`/`irun` only: `iq`'s stdout is its return value and `ied`'s is its
//! report, so neither can hand a script a second, uncoordinated writer onto
//! the same streams.

use mlua::Lua;
use std::io::Write;

/// `tostring`-style rendering for `io.*` arguments (matches `print`).
fn display_value(v: &mlua::Value) -> String {
    v.to_string().unwrap_or_else(|e| format!("<error: {e}>"))
}

/// Register the `io` module on `lua`. Host-only (`ito`/`irun`).
pub fn register_host(lua: &Lua) -> mlua::Result<()> {
    let module = lua.create_table()?;

    macro_rules! stream_fn {
        ($name:literal, $stream:ident, $newline:expr) => {
            module.set(
                $name,
                lua.create_function(move |_, args: mlua::Variadic<mlua::Value>| {
                    let mut out = std::io::$stream().lock();
                    let parts: Vec<String> = args.iter().map(display_value).collect();
                    let text = parts.join("\t");
                    let result = if $newline {
                        writeln!(out, "{text}")
                    } else {
                        write!(out, "{text}")
                    };
                    result.map_err(mlua::Error::external)
                })?,
            )?;
        };
    }
    stream_fn!("write", stdout, false);
    stream_fn!("writeln", stdout, true);
    stream_fn!("ewrite", stderr, false);
    stream_fn!("ewriteln", stderr, true);

    lua.globals().set("io", module)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua() -> Lua {
        let lua = Lua::new();
        register_host(&lua).expect("register_host");
        lua
    }

    #[test]
    fn all_functions_callable() {
        let lua = lua();
        for name in ["write", "writeln", "ewrite", "ewriteln"] {
            lua.load(format!(r#"io.{name}("hello", 1, true)"#))
                .exec()
                .unwrap_or_else(|e| panic!("io.{name} failed: {e}"));
        }
    }
}
