//! The `fmt` module: formatting for terminal output.
//!
//! One global, `fmt`, grouping what shapes text for a reader:
//!
//! - [`ansi`] — the colour switch (`fmt.colors`, host-only `fmt.is_tty`), the
//!   styles (`fmt.red(s)` / `s:red()`, …), `if_color`/`if_nocolor` and the
//!   status symbols (`fmt.ok()`/`fail()`/`warn()`/`info()`).
//! - [`text`] — character- and escape-aware string helpers, `fmt.strip` and
//!   `fmt.width`.
//! - [`table`] — `fmt.table.new(columns)`, the aligned text table builder.
//!
//! Every function that takes a string subject is registered twice from one
//! implementation: as `fmt.<name>(s, …)` and as the `string` method
//! `s:<name>(…)`, which stays as sugar. Everything here is pure; only the
//! colour *auto-detection* and `fmt.is_tty` read the host (see
//! [`register_host`]). Table output never depends on the terminal.

use mlua::{Function, Lua, Table};

pub mod ansi;
mod table;
mod text;

pub use ansi::{enabled, enabled_stderr};

/// Serialises the tests that move the process-wide colour switch.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Register one function as `fmt.<name>` and as the `string` method `<name>`.
pub(super) fn def(fmt: &Table, string: &Table, name: &str, f: Function) -> mlua::Result<()> {
    fmt.set(name, f.clone())?;
    string.set(name, f)
}

/// Register the `fmt` global (and the `string` sugar) on every binary. The
/// caller resets the process-wide colour switch first ([`ansi::reset`]).
pub fn register(lua: &Lua) -> mlua::Result<()> {
    let fmt = lua.create_table()?;
    let string: Table = lua.globals().get("string")?;
    ansi::register(lua, &fmt, &string)?;
    text::register(lua, &fmt, &string)?;
    table::register(lua, &fmt)?;
    lua.globals().set("fmt", fmt)
}

/// Add `fmt.is_tty` and let colour auto-detection read the host. Host-only
/// (`ito`); must run before `sandbox(true)` freezes the `fmt` table.
pub fn register_host(lua: &Lua) -> mlua::Result<()> {
    let fmt: Table = lua.globals().get("fmt")?;
    ansi::register_host(lua, &fmt)
}
