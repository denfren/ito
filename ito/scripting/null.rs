//! The global `null` — the one sentinel `json`/`yaml`/`hcl` (and `yaml.edit`)
//! use for an explicit null. A Luau table cannot store `nil` as a value, so a
//! parsed `null` is this light-userdata value instead. It is the same value
//! mlua's serde layer produces for `none`/`unit` (`LuaSerdeExt::null`), which
//! is what keeps every conversion (and the aws response conversion) agreeing.
//! `toml` has no null: writing it there raises.

use mlua::{Lua, LuaSerdeExt};

/// Register the global `null`. Must run before `sandbox(true)`.
pub fn register(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set("null", lua.null())
}
