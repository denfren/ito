//! The `ito` scripting module.
//!
//! Registered as the global `ito` table. Covers:
//!
//! - `ito.version_check(req)` — abort if the running ito is incompatible
//!   with the semver requirement `req` (caret semantics on a bare version; a
//!   `0.x` minor bump is incompatible, as in Cargo). `req` must be a string.
//! - `ito.slug()` — a random `"adjective-noun"` name, host-only (see
//!   [`register_host_slug`]).
//!
//! The host locations live elsewhere: `fs.require_root()`, `fs.require_dir()`
//! and `fs.script()` sit next to `fs.cwd()` (see [`register_host_paths`]), and
//! `proc.pid()` next to the other process functions (see [`register_host_pid`]).
//!
//! Script arguments are not here: everything after `SCRIPT` on the command
//! line reaches the script as the global `argv` (see [`crate::scripting::argv`]).

// ---------------------------------------------------------------------------
// Engine registration
// ---------------------------------------------------------------------------

/// Register the `ito` module on `lua`.
pub fn register(lua: &mlua::Lua, current: &'static str) -> mlua::Result<()> {
    let module = lua.create_table()?;

    // ito.version_check(req)
    module.set(
        "version_check",
        lua.create_function(move |_, req: mlua::Value| {
            // Lua would stringify a number (`0.10` -> "0.1"), silently asking
            // for a different version: require a string.
            let mlua::Value::String(req) = req else {
                return Err(mlua::Error::runtime(format!(
                    "ito.version_check: expected a string, got {}",
                    req.type_name()
                )));
            };
            version_check(&req.to_str()?, current).map_err(mlua::Error::runtime)
        })?,
    )?;

    lua.globals().set("ito", module)?;
    Ok(())
}

/// Add `fs.require_root()`, `fs.require_dir()` and `fs.script()` to the
/// already-registered `fs` table.
///
/// The anchors `require` resolves against (the tree imports are confined to,
/// and the directory the calling file's relative imports start from), plus the
/// entry script's own path, handed to the script so it can resolve its *own*
/// paths the same way. Everything else is a `path.join` from one of them.
///
/// All three are symlink-resolved: the entry script is canonicalized first (see
/// `run::Script::file`), and when that fails the anchors silently fall back to
/// the `-C` directory.
///
/// Host-only (`ito`), the line `fs.cwd()` draws: they name locations on the
/// host filesystem, which a rooted `fs` could not act on. Absent rather than
/// stubbed, so `if fs.require_dir then` is a usable feature test.
///
/// `require_dir` is a function because it is per-chunk: it answers for
/// whichever file called it, so a `require`d module gets its own directory.
///
/// Must run after `fs` is registered and before `sandbox(true)` freezes it.
pub fn register_host_paths(
    lua: &mlua::Lua,
    root: std::path::PathBuf,
    script: std::path::PathBuf,
) -> mlua::Result<()> {
    let module: mlua::Table = lua.globals().get("fs")?;

    // fs.require_root() -> String
    let root_string = root.to_string_lossy().into_owned();
    module.set(
        "require_root",
        lua.create_function(move |_, ()| Ok(root_string.clone()))?,
    )?;

    // fs.script() -> String
    let script_string = script.to_string_lossy().into_owned();
    module.set(
        "script",
        lua.create_function(move |_, ()| Ok(script_string.clone()))?,
    )?;

    // fs.require_dir() -> String?
    // Where the calling chunk's own relative imports resolve from. A
    // filesystem-backed chunk is named `@<abs path>` (see `run::Script` and
    // `scripting::require`), so this is its parent; for a required `init`
    // module that is the package directory itself.
    //
    // `nil` where there is no directory to name: a chunk not named after a
    // file (`=<label>`), or a call from a host-spawned context (or through a
    // `pcall`) with no Luau frame of its own above it.
    module.set(
        "require_dir",
        lua.create_function(|lua, ()| {
            // Level 1 is the caller of this function, the chunk we answer for.
            let Some(chunk) = lua.inspect_stack(1, |d| d.source().source.map(|s| s.to_string()))
            else {
                return Ok(None);
            };
            Ok(chunk.and_then(|c| chunk_dir(&c)))
        })?,
    )?;

    Ok(())
}

/// Add `proc.pid()` to the already-registered `proc` table.
///
/// Host-only (`ito`) for the reason `os.getenv` is: a pid is ambient
/// state the caller never passed, and it differs between two runs of one
/// script — exactly what an `ied --diff` preview must not read.
///
/// Must run after `proc::register` and before `sandbox(true)`.
pub fn register_host_pid(lua: &mlua::Lua) -> mlua::Result<()> {
    let module: mlua::Table = lua.globals().get("proc")?;
    // `std::process::id`: `u32`, so Luau sees a plain integer.
    module.set("pid", lua.create_function(|_, ()| Ok(std::process::id()))?)?;
    Ok(())
}

/// Add `ito.slug()` to the already-registered `ito` table.
///
/// Host-only (`ito`), like `proc.pid`: it's random, and `math.random` is
/// dropped from the sandbox for the same reason -- a script whose output must
/// be reproducible (an `ied` preview) cannot see it.
///
/// Returns a `"adjective-noun"` pair (e.g. `"rusty-nail"`) from the `names`
/// crate's default generator, for scratch directory/resource names that read
/// better than a random hex string. Not a general-purpose ID: no numbered
/// variant, no custom word lists, no uniqueness guarantee.
///
/// Must run before `sandbox(true)` freezes the `ito` table.
pub fn register_host_slug(lua: &mlua::Lua) -> mlua::Result<()> {
    let module: mlua::Table = lua.globals().get("ito")?;
    module.set(
        "slug",
        lua.create_function(|_, ()| {
            Ok(names::Generator::default()
                .next()
                .expect("names::Generator never returns None"))
        })?,
    )?;
    Ok(())
}

/// The directory holding the file a Luau chunk name refers to, if it names one.
fn chunk_dir(chunk_name: &str) -> Option<String> {
    // Only `@`-prefixed chunks are filesystem-backed; `=<label>` is not.
    let path = std::path::Path::new(chunk_name.strip_prefix('@')?);
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// version_check internals
// ---------------------------------------------------------------------------

/// A bare `X`, `X.Y`, or `X.Y.Z` literal is read as a caret requirement;
/// anything containing an operator is taken verbatim.
fn version_check(req: &str, current: &str) -> Result<(), String> {
    let version = semver::Version::parse(current).map_err(|e| {
        format!("ito.version_check: internal: bad running version {current:?}: {e}")
    })?;
    let requirement = semver::VersionReq::parse(req).map_err(|e| {
        format!("ito.version_check: {req:?} is not a valid version requirement: {e}")
    })?;
    if requirement.matches(&version) {
        Ok(())
    } else {
        Err(format!(
            "ito.version_check: this script requires ito {req} (compatible with \
             {requirement}), but the running ito is {current}; \
             upgrade ito or relax the requirement",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_within_leading_nonzero_matches() {
        assert!(version_check("0.2.1", "0.2.1").is_ok());
        assert!(version_check("0.2.1", "0.2.9").is_ok());
        assert!(version_check("0.2", "0.2.5").is_ok());
    }

    #[test]
    fn caret_rejects_breaking_bumps() {
        assert!(version_check("0.2.1", "0.3.0").is_err());
        assert!(version_check("0.2.1", "0.2.0").is_err());
        assert!(version_check("0.2.1", "1.0.0").is_err());
        assert!(version_check("1.2.3", "2.0.0").is_err());
    }

    #[test]
    fn one_x_series_allows_forward_minor() {
        assert!(version_check("1.2.3", "1.2.3").is_ok());
        assert!(version_check("1.2.3", "1.9.0").is_ok());
        assert!(version_check("1.2.3", "1.2.2").is_err());
    }

    #[test]
    fn explicit_requirement_strings() {
        assert!(version_check(">=0.2, <0.4", "0.3.0").is_ok());
        assert!(version_check(">=0.2, <0.4", "0.4.0").is_err());
        assert!(version_check("^0.2.1", "0.2.9").is_ok());
    }

    #[test]
    fn invalid_requirement_is_an_error() {
        assert!(version_check("not-a-version", "0.1.0").is_err());
    }

    #[test]
    fn chunk_dir_is_the_parent_of_the_named_path() {
        // Luau strips the extension from the chunk name; the parent is the
        // same either way, which is why nothing has to put one back.
        assert_eq!(
            chunk_dir("@/proj/sub/lib").as_deref(),
            Some("/proj/sub"),
            "extension-stripped chunk name"
        );
        assert_eq!(chunk_dir("@/proj/main.luau").as_deref(), Some("/proj"));
    }

    #[test]
    fn chunk_dir_declines_chunks_that_name_no_file() {
        // `=<label>`: not filesystem-backed, so no directory.
        assert_eq!(chunk_dir("=<label>"), None);
        // A bare name has no parent worth reporting (`""`, not a directory).
        assert_eq!(chunk_dir("@main"), None);
    }

    #[test]
    fn version_check_requires_a_string() {
        let lua = mlua::Lua::new();
        register(&lua, "0.10.0").unwrap();
        let err = lua.load("ito.version_check(0.10)").exec().unwrap_err();
        assert!(err.to_string().contains("expected a string"), "{err}");
        lua.load(r#"ito.version_check("0.10")"#).exec().unwrap();
    }

    #[test]
    fn host_paths_land_on_fs_and_pid_on_proc() {
        let lua = mlua::Lua::new();
        lua.globals().set("fs", lua.create_table().unwrap()).unwrap();
        lua.globals().set("proc", lua.create_table().unwrap()).unwrap();
        register_host_paths(&lua, "/r".into(), "/r/main.luau".into()).unwrap();
        register_host_pid(&lua).unwrap();
        lua.load(
            r#"
            assert(fs.require_root() == "/r")
            assert(fs.script() == "/r/main.luau")
            assert(proc.pid() > 0)
        "#,
        )
        .exec()
        .unwrap();
    }

    #[test]
    fn error_message_mentions_both_versions() {
        let err = version_check("0.3.0", "0.2.1").unwrap_err();
        assert!(err.contains("0.3.0"), "{err}");
        assert!(err.contains("0.2.1"), "{err}");
    }
}
