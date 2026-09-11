//! The `argv` global: the script's own arguments.
//!
//! The binary's own flags come before `SCRIPT`; everything after it belongs to
//! the script, not to the binary:
//!
//! ```text
//! ito -v deploy.luau --env prod --dry-run
//! ```
//!
//! clap stops interpreting at `SCRIPT`, so the tail is passed through verbatim
//! as a list of strings and registered here as the global `argv`.
//!
//! Unlike a C `argv`, **element 1 is not the program name** — only the strings
//! given after `SCRIPT` appear, so `argv[1]` is the script's first argument and
//! `#argv` is how many were passed. A script that got none sees an empty table,
//! never nil.
//!
//! Parsing them is the script's job: there is no option map, no type coercion
//! and no name validation, because every script wants a different grammar and
//! guessing one only gets in the way.

/// The script's arguments, everything after `SCRIPT` on the command line.
#[derive(Debug, Clone, Default)]
pub struct Argv(pub Vec<String>);

/// Register the `argv` global on `lua`.
pub fn register(lua: &mlua::Lua, argv: Argv) -> mlua::Result<()> {
    lua.globals()
        .set("argv", lua.create_sequence_from(argv.0)?)?;
    Ok(())
}
