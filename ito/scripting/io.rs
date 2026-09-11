//! `io` module: the standard streams as file-like handles (`ito` only).
//!
//! - `io.stdout`, `io.stderr`, `io.stdin` — handles with `:write(...)`,
//!   `:flush()`, `:close()` and `:setvbuf(mode)`; `io.stdin` also has
//!   `:read(fmt)` and `:lines()`. `:write` returns the handle, so calls chain,
//!   and is strict like vanilla Lua: only strings and numbers, anything else
//!   raises (use `print` or `tostring` for the rest). `:close()` raises on a
//!   standard handle; `:setvbuf` is accepted and does nothing, because every
//!   write is flushed.
//! - `io.write`, `io.read`, `io.lines`, `io.flush` — the same calls on the
//!   default handles (`io.stdout` for output, `io.stdin` for input).
//!
//! Deliberately absent: `io.open` and the path form of `io.lines` (`fs` owns
//! files) and the `io.output`/`io.input` setters (a `proc` redirect covers
//! that). Every write goes through [`super::print::emit`], the same sink as
//! `print`.
//!
//! `ied` has no `io`: `print` is its whole output surface.

use mlua::{AnyUserData, Lua, MetaMethod, UserData, UserDataMethods, Value, Variadic};
use std::io::{BufRead, Read};

use super::print::{Out, emit, tostring_bytes};

#[derive(Clone, Copy)]
enum Handle {
    Stdout,
    Stderr,
    Stdin,
}

impl Handle {
    fn name(self) -> &'static str {
        match self {
            Handle::Stdout => "stdout",
            Handle::Stderr => "stderr",
            Handle::Stdin => "stdin",
        }
    }
    fn out(self) -> Option<Out> {
        match self {
            Handle::Stdout => Some(Out::Stdout),
            Handle::Stderr => Some(Out::Stderr),
            Handle::Stdin => None,
        }
    }
}

/// A read request, parsed from a vanilla format.
enum Fmt {
    All,
    Line { keep_newline: bool },
    Count(usize),
}

fn parse_fmt(v: Value) -> mlua::Result<Fmt> {
    match v {
        Value::Nil => Ok(Fmt::Line { keep_newline: false }),
        Value::Integer(n) if n >= 0 => Ok(Fmt::Count(n as usize)),
        Value::Number(n) if n >= 0.0 => Ok(Fmt::Count(n as usize)),
        Value::String(s) => {
            let s = s.to_str()?;
            match s.strip_prefix('*').unwrap_or(&s) {
                "a" => Ok(Fmt::All),
                "l" => Ok(Fmt::Line { keep_newline: false }),
                "L" => Ok(Fmt::Line { keep_newline: true }),
                _ => Err(mlua::Error::runtime(format!(
                    "bad argument #1 to 'read' (invalid format '{s}'; expected \"a\", \"l\", \"L\" or a count)"
                ))),
            }
        }
        other => Err(mlua::Error::runtime(format!(
            "bad argument #1 to 'read' (string or number expected, got {})",
            other.type_name()
        ))),
    }
}

/// Read per `fmt` on the blocking pool. `None` is EOF (`"a"` never is: it gives
/// `""`, like vanilla).
async fn read(fmt: Fmt) -> mlua::Result<Option<Vec<u8>>> {
    tokio::task::spawn_blocking(move || -> std::io::Result<Option<Vec<u8>>> {
        let mut stdin = std::io::stdin().lock();
        let mut buf = Vec::new();
        match fmt {
            Fmt::All => {
                stdin.read_to_end(&mut buf)?;
            }
            Fmt::Count(n) => {
                stdin.take(n as u64).read_to_end(&mut buf)?;
                // `read(0)` probes for EOF rather than reading nothing.
                if n == 0 {
                    return Ok(if stdin_at_eof()? { None } else { Some(buf) });
                }
                if buf.is_empty() {
                    return Ok(None);
                }
            }
            Fmt::Line { keep_newline } => {
                if stdin.read_until(b'\n', &mut buf)? == 0 {
                    return Ok(None);
                }
                if !keep_newline && buf.last() == Some(&b'\n') {
                    buf.pop();
                    if buf.last() == Some(&b'\r') {
                        buf.pop();
                    }
                }
            }
        }
        Ok(Some(buf))
    })
    .await
    .map_err(mlua::Error::external)?
    .map_err(mlua::Error::external)
}

fn stdin_at_eof() -> std::io::Result<bool> {
    Ok(std::io::stdin().lock().fill_buf()?.is_empty())
}

fn to_value(lua: &Lua, bytes: Option<Vec<u8>>) -> mlua::Result<Value> {
    match bytes {
        Some(b) => Ok(Value::String(lua.create_string(b)?)),
        None => Ok(Value::Nil),
    }
}

fn write_args(
    tostring: &mlua::Function,
    handle: Handle,
    args: Variadic<Value>,
) -> mlua::Result<()> {
    let Some(out) = handle.out() else {
        return Err(mlua::Error::runtime("cannot write to io.stdin"));
    };
    let mut bytes = Vec::new();
    for (i, v) in args.into_iter().enumerate() {
        match v {
            Value::String(s) => bytes.extend_from_slice(&s.as_bytes()),
            v @ (Value::Integer(_) | Value::Number(_)) => {
                bytes.extend(tostring_bytes(tostring, v)?)
            }
            other => {
                return Err(mlua::Error::runtime(format!(
                    "bad argument #{} to 'write' (string expected, got {})",
                    i + 1,
                    other.type_name()
                )));
            }
        }
    }
    emit(out, &bytes)
}

impl UserData for Handle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            Ok(format!("file (io.{})", this.name()))
        });
        methods.add_function(
            "write",
            |lua, (ud, args): (AnyUserData, Variadic<Value>)| {
                let handle = *ud.borrow::<Handle>()?;
                let tostring: mlua::Function = lua.globals().get("tostring")?;
                write_args(&tostring, handle, args)?;
                Ok(ud)
            },
        );
        methods.add_function("flush", |_, ud: AnyUserData| {
            // Every write is already flushed.
            Ok(ud)
        });
        methods.add_method("close", |_, this, ()| -> mlua::Result<()> {
            Err(mlua::Error::runtime(format!(
                "cannot close standard file io.{}",
                this.name()
            )))
        });
        methods.add_method("setvbuf", |_, _, _: Variadic<Value>| Ok(true));
        methods.add_async_method("read", async |lua, this, fmt: Value| {
            if !matches!(*this, Handle::Stdin) {
                return Err(mlua::Error::runtime(format!(
                    "cannot read from io.{}",
                    this.name()
                )));
            }
            to_value(&lua, read(parse_fmt(fmt)?).await?)
        });
        methods.add_method("lines", |lua, this, ()| {
            if !matches!(*this, Handle::Stdin) {
                return Err(mlua::Error::runtime(format!(
                    "cannot read from io.{}",
                    this.name()
                )));
            }
            lines(lua)
        });
    }
}

fn lines(lua: &Lua) -> mlua::Result<mlua::Function> {
    lua.create_async_function(async |lua, ()| {
        to_value(&lua, read(Fmt::Line { keep_newline: false }).await?)
    })
}

/// Register the `io` table (`ito` only).
pub fn register(lua: &Lua) -> mlua::Result<()> {
    let module = lua.create_table()?;
    let stdout = lua.create_userdata(Handle::Stdout)?;
    let stderr = lua.create_userdata(Handle::Stderr)?;
    let stdin = lua.create_userdata(Handle::Stdin)?;
    module.set("stdout", stdout.clone())?;
    module.set("stderr", stderr)?;
    module.set("stdin", stdin)?;

    module.set(
        "write",
        lua.create_function(move |lua, args: Variadic<Value>| {
            let tostring: mlua::Function = lua.globals().get("tostring")?;
            write_args(&tostring, Handle::Stdout, args)?;
            Ok(stdout.clone())
        })?,
    )?;
    module.set("flush", lua.create_function(|_, ()| Ok(()))?)?;
    module.set(
        "read",
        lua.create_async_function(async |lua, fmt: Value| {
            to_value(&lua, read(parse_fmt(fmt)?).await?)
        })?,
    )?;
    module.set("lines", lua.create_function(|lua, ()| lines(lua))?)?;

    lua.globals().set("io", module)?;
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
    fn writes_chain_and_take_strings_and_numbers() {
        lua()
            .load(
                r#"io.stdout:write("a", 1, 2.5):write("\n")
                   io.stderr:write("e\n"):flush()
                   io.write("w\n")
                   io.flush()"#,
            )
            .exec()
            .unwrap();
    }

    #[test]
    fn write_is_strict() {
        let lua = lua();
        for bad in ["nil", "true", "{}"] {
            let r = lua.load(format!("io.stdout:write({bad})")).exec();
            assert!(r.is_err(), "write({bad}) should raise");
        }
    }

    #[test]
    fn close_raises_and_setvbuf_is_accepted() {
        let lua = lua();
        assert!(lua.load("io.stdout:close()").exec().is_err());
        lua.load(r#"io.stdout:setvbuf("no")"#).exec().unwrap();
    }

    #[test]
    fn wrong_direction_raises() {
        let lua = lua();
        assert!(lua.load("io.stdin:write('x')").exec().is_err());
        assert!(lua.load("io.stdout:lines()").exec().is_err());
    }

    #[test]
    fn no_open_and_no_setters_and_no_ewrite() {
        let lua = lua();
        for name in ["open", "output", "input", "ewrite", "ewriteln", "writeln"] {
            let v: Value = lua.load(format!("return io.{name}")).eval().unwrap();
            assert!(v.is_nil(), "io.{name} must not exist");
        }
    }
}
