//! Interactive terminal UI: `ui.prompt` and `ui.progress`.
//!
//! Registered as the global `ui` table, holding two namespaces:
//!
//! - [`prompt`] — `dialoguer`-backed interactive prompts (confirm, input,
//!   password, select, multi-select).
//! - [`progress`] — `indicatif`-backed progress bars and spinners.
//!
//! Both read or write the terminal directly, which is ambient state a script
//! under `iq`/`ied` cannot be allowed to depend on — a query's output is its
//! return value, and an `ied --diff` preview must not block on a prompt or
//! print a bar that has no counterpart under `--write`. So `ui` is host-only
//! (`ito`/`irun`), the same line `proc`/`ssh`/`task` draw, and is registered
//! under `host_io` rather than with the core. Absent rather than stubbed, so
//! `if ui then` is a usable feature test.
//!
//! More namespaces (`ui.table`, `ui.tree`, …) are expected to join these two
//! later; `ui` is a namespace of namespaces from the start rather than a flat
//! module, so a later addition costs a new submodule, not a rename of this one.

pub mod progress;
pub mod prompt;

use mlua::{Lua, Result as LuaResult, Table};

/// Register the `ui` module: an empty umbrella table, then `prompt` and
/// `progress` under it.
///
/// Host-only (`ito`/`irun`), like `proc`/`ssh`/`task`. Must run before
/// `sandbox(true)` freezes the table, matching every other `host_io` step in
/// `Engine::build`.
pub fn register(lua: &Lua) -> LuaResult<()> {
    let module = lua.create_table()?;

    let prompt_table: Table = prompt::module(lua)?;
    module.set("prompt", prompt_table)?;

    let progress_table: Table = progress::module(lua)?;
    module.set("progress", progress_table)?;

    lua.globals().set("ui", module)?;
    Ok(())
}
