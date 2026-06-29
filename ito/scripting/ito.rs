//! The `ito` scripting module.
//!
//! Registered as the global `ito` table. Covers:
//!
//! - `ito.version_check(req)` — abort if the running ito is incompatible
//!   with the semver requirement `req` (caret semantics on a bare version).
//! - `ito.null` — the sentinel value `json`/`toml`/`yaml`/`hcl` use for an
//!   explicit null (a Lua table cannot store `nil` as a value, so a parsed
//!   `null` is not plain `nil` — see [`crate::scripting::json`]).
//! - `ito.is_null(value)` → `bool` — true iff `value` is that sentinel.
//!
//! Two more exist **only under `ito`/`irun`** (see [`register_host_paths`]),
//! because they name host locations:
//!
//! - `ito.require_root()` — the outer bound, absolute: the directory imports
//!   are confined to.
//! - `ito.require_dir()` — the directory the *calling* chunk's relative
//!   imports resolve against, absolute, so a `require`d module gets its own
//!   rather than the entry script's.
//! - `ito.pid()` — the running process's own pid (see [`register_host_pid`]).
//! - `ito.slug()` — a random `"adjective-noun"` name (see
//!   [`register_host_slug`]).
//!
//! The two anchors `require` itself uses, which is why they share its name: one
//! script wants "relative to the whole tree", another "relative to me".
//!
//! Script arguments are not here: everything after `::` on the command line
//! reaches the script as the global `argv` (see [`crate::scripting::argv`]).

// ---------------------------------------------------------------------------
// Engine registration
// ---------------------------------------------------------------------------

/// Register the `ito` module on `lua`.
pub fn register(lua: &mlua::Lua, current: &'static str) -> mlua::Result<()> {
    let module = lua.create_table()?;

    // ito.version_check(req)
    module.set(
        "version_check",
        lua.create_function(move |_, req: String| {
            version_check(&req, current).map_err(mlua::Error::runtime)
        })?,
    )?;

    // ito.null — the shared null sentinel (see json/toml/yaml/hcl).
    module.set("null", mlua::LuaSerdeExt::null(lua))?;

    // ito.is_null(value) -> bool
    module.set(
        "is_null",
        lua.create_function(|lua, value: mlua::Value| Ok(value == mlua::LuaSerdeExt::null(lua)))?,
    )?;

    lua.globals().set("ito", module)?;
    Ok(())
}

/// Add `ito.require_root()` and `ito.require_dir()` to the already-registered
/// `ito` table.
///
/// The two anchors `require` resolves against, handed to the script so it can
/// resolve its *own* paths the same way: the tree imports are confined to, and
/// the directory the calling file's relative imports start from. Everything
/// else is a `path.join` from one of them.
///
/// Host-only (`ito`/`irun`): both name a location on the host filesystem, which
/// is the same line `fs.cwd()` draws. Under `iq`/`ied` the require root is a
/// boundary the script is not meant to locate, and a path it could not act on
/// through its own rooted `fs` would be a leak rather than a feature. Absent
/// rather than stubbed, so `if ito.require_dir then` is a usable feature test.
///
/// Both are functions rather than constants because only one of them *could*
/// have been a constant: `require_dir` is per-chunk, answering for whichever
/// file called it, so a `require`d module gets its own. Spelling them the same
/// way keeps the pair from reading as two unrelated things.
///
/// Like the other reach-into-the-stdlib steps in `Engine::build`, this must run
/// before `sandbox(true)` freezes the `ito` table.
pub fn register_host_paths(lua: &mlua::Lua, root: Option<std::path::PathBuf>) -> mlua::Result<()> {
    let module: mlua::Table = lua.globals().get("ito")?;

    // ito.require_root() -> String?
    // `nil` for a stdin/-s script run without `-r`, which has no root and no
    // imports either.
    let root_string = root.map(|p| p.to_string_lossy().into_owned());
    module.set(
        "require_root",
        lua.create_function(move |_, ()| Ok(root_string.clone()))?,
    )?;

    // ito.require_dir() -> String?
    // Where the calling chunk's own relative imports resolve from. Luau names
    // a filesystem-backed chunk `@<abs path minus extension>` (see
    // `run::Script` and `scripting::require`), so this is its parent -- the
    // stripped extension does not matter to a `pop`, which is why nothing has
    // to put one back.
    //
    // `nil` where there is no directory to name: a stdin/-s chunk with no root
    // (named `=<label>`, not a path), or a call from a host-spawned context
    // with no Luau frame above it.
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

/// Add `ito.pid()` to the already-registered `ito` table.
///
/// Host-only (`ito`/`irun`) for the reason `os.getenv` is: a pid is ambient
/// state the caller never passed, and it differs between two runs of one
/// script — exactly what an `ied --diff` preview must not read, since a report
/// that embedded one could not match the `--write` after it.
///
/// A function rather than a constant, matching `require_root`/`require_dir`, so
/// the whole host-only half of this table reads as one thing.
///
/// Must run before `sandbox(true)` freezes the `ito` table.
pub fn register_host_pid(lua: &mlua::Lua) -> mlua::Result<()> {
    let module: mlua::Table = lua.globals().get("ito")?;
    // `std::process::id`, not `getpid(2)`: the same number, from the standard
    // library, and it is `u32` so Luau sees a plain integer.
    module.set("pid", lua.create_function(|_, ()| Ok(std::process::id()))?)?;
    Ok(())
}

/// Add `ito.slug()` to the already-registered `ito` table.
///
/// Host-only (`ito`/`irun`), like `ito.pid`: it's random, and `math.random` is
/// dropped from the sandbox for the same reason -- a script whose output must
/// be reproducible (`iq`, or an `ied` preview) cannot see it.
///
/// Returns a `"adjective-noun"` pair (e.g. `"rusty-nail"`) from the `names`
/// crate's default generator -- the same one `smic`'s `gen_random_tmp` uses --
/// for scratch directory/resource names that read better than a random hex
/// string. Not a general-purpose ID: no numbered variant, no custom word
/// lists, no uniqueness guarantee.
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
    // Only `@`-prefixed chunks are filesystem-backed; `=<stdin>` is not.
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
        format!("ito::version_check: internal: bad running version {current:?}: {e}")
    })?;
    let requirement = semver::VersionReq::parse(req).map_err(|e| {
        format!("ito::version_check: {req:?} is not a valid version requirement: {e}")
    })?;
    if requirement.matches(&version) {
        Ok(())
    } else {
        Err(format!(
            "ito::version_check: this script requires ito {req} (compatible with \
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
        // `=<expression>`/`=<stdin>`: not filesystem-backed, so no directory.
        assert_eq!(chunk_dir("=<expression>"), None);
        assert_eq!(chunk_dir("=<stdin>"), None);
        // A bare name has no parent worth reporting (`""`, not a directory).
        assert_eq!(chunk_dir("@main"), None);
    }

    #[test]
    fn error_message_mentions_both_versions() {
        let err = version_check("0.3.0", "0.2.1").unwrap_err();
        assert!(err.contains("0.3.0"), "{err}");
        assert!(err.contains("0.2.1"), "{err}");
    }
}
