//! `ito` module: runtime version guard for scripts.
//!
//! `ito::version_check(req)` lets a script declare the `ito` version it
//! was authored against and abort early when the running binary is not
//! compatible — a lightweight, in-script "lock file".
//!
//! `req` is a version literal like `"0.2.1"`, interpreted with caret
//! (`^`) semantics against the running `ito` version (the leftmost
//! non-zero component is the breaking axis): `"0.2.1"` accepts
//! `>=0.2.1, <0.3.0`; `"1.2.3"` accepts `>=1.2.3, <2.0.0`. Any string
//! `semver` accepts as a `VersionReq` also works (e.g. `">=0.2, <0.4"`),
//! but a bare version is the common case.
//!
//! On a mismatch the function throws a descriptive runtime error
//! (script-attributable, exit `3`); on a match it returns nothing.

use rhai::{Engine, EvalAltResult, ImmutableString, Module};

/// Register the `ito` module on `engine`, bound to the running version.
pub fn register(engine: &mut Engine, current: &'static str) {
    let mut module = Module::new();

    // ito::version_check(req)
    // Throw unless the running ito version is semver-compatible with `req`
    // (a version literal read as a caret requirement).
    module.set_native_fn(
        "version_check",
        move |req: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            check(&req, current).map_err(Into::into)
        },
    );

    engine.register_static_module("ito", module.into());
}

/// A bare `X`, `X.Y`, or `X.Y.Z` literal is read as a caret requirement;
/// anything containing an operator (`^`, `>=`, `~`, `*`, `,`, …) is taken
/// verbatim. `semver`'s `VersionReq` defaults a bare version to caret
/// already, so we only need to pass it through.
fn check(req: &str, current: &str) -> Result<(), String> {
    let version = semver::Version::parse(current)
        .map_err(|e| format!("ito::version_check: internal: bad running version {current:?}: {e}"))?;
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
        // 0.x: minor is the breaking axis, patch is forward-compatible.
        assert!(check("0.2.1", "0.2.1").is_ok());
        assert!(check("0.2.1", "0.2.9").is_ok());
        // bare partials default to caret too.
        assert!(check("0.2", "0.2.5").is_ok());
    }

    #[test]
    fn caret_rejects_breaking_bumps() {
        assert!(check("0.2.1", "0.3.0").is_err()); // minor bump (0.x break)
        assert!(check("0.2.1", "0.2.0").is_err()); // older than required
        assert!(check("0.2.1", "1.0.0").is_err()); // major bump
        assert!(check("1.2.3", "2.0.0").is_err());
    }

    #[test]
    fn one_x_series_allows_forward_minor() {
        assert!(check("1.2.3", "1.2.3").is_ok());
        assert!(check("1.2.3", "1.9.0").is_ok());
        assert!(check("1.2.3", "1.2.2").is_err());
    }

    #[test]
    fn explicit_requirement_strings() {
        assert!(check(">=0.2, <0.4", "0.3.0").is_ok());
        assert!(check(">=0.2, <0.4", "0.4.0").is_err());
        assert!(check("^0.2.1", "0.2.9").is_ok());
    }

    #[test]
    fn invalid_requirement_is_an_error() {
        assert!(check("not-a-version", "0.1.0").is_err());
    }

    #[test]
    fn error_message_mentions_both_versions() {
        let err = check("0.3.0", "0.2.1").unwrap_err();
        assert!(err.contains("0.3.0"), "{err}");
        assert!(err.contains("0.2.1"), "{err}");
    }
}
