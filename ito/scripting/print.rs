//! `print` and the one sink every script write to stdout/stderr goes through.
//!
//! `print` is vanilla: `tostring` on each argument (so `__tostring` is
//! honoured and a plain table renders as `table: 0x…`), joined with tabs, a
//! newline appended, written to stdout. It is never rerouted to the log.
//!
//! [`emit`] is the single sink, shared with the `io` handles. It pauses live
//! `ui.progress` bars, flushes every write (so output interleaves correctly
//! with log lines and bars, unlike vanilla's buffering), and turns a closed
//! pipe (`| head`) into a quiet exit `141` (128 + SIGPIPE): the run was cut
//! short, and whoever reads the exit code should be able to tell.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use mlua::{Lua, Value};

/// Exit status of a run whose output pipe went away: 128 + SIGPIPE, as a shell
/// reports a process killed by it.
pub const SIGPIPE_EXIT: i32 = 141;

/// Set once a write hit a broken pipe, so the run still ends with
/// [`SIGPIPE_EXIT`] when the script `pcall`ed the error away.
static PIPE_BROKEN: AtomicBool = AtomicBool::new(false);

pub fn pipe_broken() -> bool {
    PIPE_BROKEN.load(Ordering::Relaxed)
}

/// Clear the flag; `Engine::build` calls this so two runs in one process do
/// not inherit each other's state.
pub fn reset() {
    PIPE_BROKEN.store(false, Ordering::Relaxed);
}

/// Which standard stream a write targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Out {
    Stdout,
    Stderr,
}

/// Write `bytes` to `stream` with the bars paused, flushed. A broken pipe
/// unwinds the run quietly with exit [`SIGPIPE_EXIT`] (the `cli` exit sentinel,
/// so tasks still drain and `ied` skips its flush); any other error is raised.
pub(crate) fn emit(stream: Out, bytes: &[u8]) -> mlua::Result<()> {
    let r = super::ui::progress::suspended(|| match stream {
        Out::Stdout => {
            let mut o = std::io::stdout().lock();
            o.write_all(bytes).and_then(|_| o.flush())
        }
        Out::Stderr => {
            let mut o = std::io::stderr().lock();
            o.write_all(bytes).and_then(|_| o.flush())
        }
    });
    match r {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
            PIPE_BROKEN.store(true, Ordering::Relaxed);
            Err(super::cli::exit_error(SIGPIPE_EXIT))
        }
        r => r.map_err(mlua::Error::external),
    }
}

/// `tostring(v)` as the script sees it, as bytes (a string need not be UTF-8).
pub(crate) fn tostring_bytes(tostring: &mlua::Function, v: Value) -> mlua::Result<Vec<u8>> {
    let s: mlua::LuaString =tostring.call(v)?;
    Ok(s.as_bytes().to_vec())
}

/// Install the vanilla `print` global.
///
/// Captures the `tostring` that exists now, so a script reassigning the global
/// cannot change what `print` renders.
///
/// With `enabled` false (`ied -q`, `-d`, `-l`) the arguments are not even
/// rendered: `print` is a silent no-op.
pub fn register(lua: &Lua, enabled: bool) -> mlua::Result<()> {
    let tostring: mlua::Function = lua.globals().get("tostring")?;
    let print = lua.create_function(move |_, args: mlua::Variadic<Value>| {
        if !enabled {
            return Ok(());
        }
        let mut out = Vec::new();
        for (i, v) in args.into_iter().enumerate() {
            if i > 0 {
                out.push(b'\t');
            }
            out.extend(tostring_bytes(&tostring, v)?);
        }
        out.push(b'\n');
        emit(Out::Stdout, &out)
    })?;
    lua.globals().set("print", print)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_is_callable_and_honours_tostring() {
        let lua = Lua::new();
        register(&lua, true).unwrap();
        lua.load(
            r#"print("a", 1, true, nil)
               print(setmetatable({}, {__tostring = function() return "x" end}))"#,
        )
        .exec()
        .unwrap();
    }

    #[test]
    fn disabled_print_is_a_silent_no_op() {
        let lua = Lua::new();
        register(&lua, false).unwrap();
        // Not even rendered: a failing `__tostring` is never called.
        lua.load("print(setmetatable({}, {__tostring = function() error('boom') end}))")
            .exec()
            .unwrap();
    }

    #[test]
    fn a_failing_tostring_raises() {
        let lua = Lua::new();
        register(&lua, true).unwrap();
        let r = lua
            .load("print(setmetatable({}, {__tostring = function() error('boom') end}))")
            .exec();
        assert!(r.is_err());
    }
}
