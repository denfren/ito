//! The `ito` scripting module.
//!
//! Registered as `ito::` on the engine. Covers:
//!
//! - `ito::version_check(req)` — abort if the running ito is incompatible
//!   with the semver requirement `req` (caret semantics on a bare version).
//! - `ito::flag(name)` → `bool` — true when `+name` was passed on the CLI.
//! - `ito::opt(name)` → `String` — value of `name=value`; throws if absent.
//! - `ito::try_opt(name)` → `String | ()` — value or unit when absent.
//!
//! Script flags (`key=value` / `+flag`) are stripped from the raw argument
//! list before clap sees it by [`ScriptFlags::split`].

use std::collections::{HashMap, HashSet};

use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Module};

// ---------------------------------------------------------------------------
// ScriptFlags — parsed before clap
// ---------------------------------------------------------------------------

fn is_script_key(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c == '-' || c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Script flags extracted from the raw argument list.
#[derive(Debug, Clone, Default)]
pub struct ScriptFlags {
    /// `key=value` pairs.
    pub opts: HashMap<String, String>,
    /// `+flag` boolean flags (stored without the `+` prefix).
    pub flags: HashSet<String>,
}

impl ScriptFlags {
    /// Partition `args` into script flags and the remainder for clap.
    ///
    /// - `+name` → boolean flag (stored without `+`); `name` must be non-empty.
    /// - `key=value` where `key` matches `[a-z_][a-z0-9_-]*` → option pair.
    ///   Tokens like `--flag=val`, `let x = y`, or `PATH=/bin` are left for clap.
    pub fn split(args: &[String]) -> (Self, Vec<String>) {
        let mut this = Self::default();
        let mut rest = Vec::new();

        for arg in args {
            if let Some(name) = arg.strip_prefix('+') {
                if !name.is_empty() {
                    this.flags.insert(name.to_string());
                    continue;
                }
            }
            if let Some((k, v)) = arg.split_once('=') {
                if is_script_key(k) {
                    this.opts.insert(k.to_string(), v.to_string());
                    continue;
                }
            }
            rest.push(arg.clone());
        }

        (this, rest)
    }
}

// ---------------------------------------------------------------------------
// Engine registration
// ---------------------------------------------------------------------------

/// Register the `ito` module on `engine`.
pub fn register(engine: &mut Engine, current: &'static str, script_flags: ScriptFlags) {
    let mut module = Module::new();

    // ito::version_check(req)
    module.set_native_fn(
        "version_check",
        move |req: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            version_check(&req, current).map_err(Into::into)
        },
    );

    // ito::flag(name) -> bool
    let flags = script_flags.flags.clone();
    module.set_native_fn(
        "flag",
        move |name: ImmutableString| -> Result<bool, Box<EvalAltResult>> {
            Ok(flags.contains(name.as_str()))
        },
    );

    // ito::opt(name) -> String   (throws when absent)
    let opts = script_flags.opts.clone();
    module.set_native_fn(
        "opt",
        move |name: ImmutableString| -> Result<ImmutableString, Box<EvalAltResult>> {
            opts.get(name.as_str())
                .map(|v| ImmutableString::from(v.as_str()))
                .ok_or_else(|| format!("ito::opt: no option {name:?} was passed").into())
        },
    );

    // ito::try_opt(name) -> String | ()
    let opts = script_flags.opts.clone();
    module.set_native_fn(
        "try_opt",
        move |name: ImmutableString| -> Result<Dynamic, Box<EvalAltResult>> {
            Ok(opts
                .get(name.as_str())
                .map(|v| Dynamic::from(ImmutableString::from(v.as_str())))
                .unwrap_or(Dynamic::UNIT))
        },
    );

    engine.register_static_module("ito", module.into());
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
    fn error_message_mentions_both_versions() {
        let err = version_check("0.3.0", "0.2.1").unwrap_err();
        assert!(err.contains("0.3.0"), "{err}");
        assert!(err.contains("0.2.1"), "{err}");
    }

    #[test]
    fn split_flags_and_opts() {
        let args: Vec<String> = ["key=val", "+dry-run", "--quiet", "other"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (sf, rest) = ScriptFlags::split(&args);
        assert_eq!(sf.opts.get("key").map(|s| s.as_str()), Some("val"));
        assert!(sf.flags.contains("dry-run"));
        assert_eq!(rest, vec!["--quiet", "other"]);
    }

    #[test]
    fn flags_consumed_past_double_dash() {
        // -- is not special; +flag and key=val after it are still stripped.
        let args: Vec<String> = ["key=val", "--", "+a-flag"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (sf, rest) = ScriptFlags::split(&args);
        assert!(sf.opts.contains_key("key"));
        assert!(sf.flags.contains("a-flag"));
        assert_eq!(rest, vec!["--"]);
    }

    #[test]
    fn assignment_expression_not_treated_as_opt() {
        // "let x = y" contains '=' but key "let x " fails is_script_key.
        let args: Vec<String> = ["let x = y"].iter().map(|s| s.to_string()).collect();
        let (sf, rest) = ScriptFlags::split(&args);
        assert!(sf.opts.is_empty());
        assert_eq!(rest, vec!["let x = y"]);
    }

    #[test]
    fn clap_style_flags_are_left_alone() {
        let args: Vec<String> = ["--flag=val", "-S"].iter().map(|s| s.to_string()).collect();
        let (sf, rest) = ScriptFlags::split(&args);
        assert!(sf.opts.is_empty());
        assert!(sf.flags.is_empty());
        assert_eq!(rest, args);
    }
}
