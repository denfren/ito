//! Integration tests for the ambient-state split: `os` and `math.random` are
//! absent in `iq`/`ied` and present in `ito`, and `os.getenv` — which Luau's
//! sandboxed `os` does not ship at all — is added back for `ito` alone.
//!
//! The point is not that a call fails but that the *name* is missing, so
//! `if os then` is a usable feature test — the same contract `fs.write` has.
//! `ito` keeps both: it already spawns processes and opens sockets, so
//! withholding the clock would be a pretence. `getenv` is the one exception to
//! the table-level test, being a key that varies within a table `ito` has
//! either way, so its own test is `if os.getenv then`.

use assert_cmd::Command;
use predicates::prelude::*;

fn iq() -> Command {
    Command::cargo_bin("iq").unwrap()
}

fn ied() -> Command {
    Command::cargo_bin("ied").unwrap()
}

fn ito() -> Command {
    Command::cargo_bin("ito").unwrap()
}

/// The three names as present/absent. `iq` returns its output, so it takes
/// `return PROBE`; `ied`/`ito` print theirs, so they take `print(PROBE)`.
const PROBE: &str = r#""os=" .. typeof(os)
    .. " random=" .. typeof(math.random)
    .. " randomseed=" .. typeof(math.randomseed)"#;

// --- denied in the two deterministic binaries ---

#[test]
fn iq_has_no_ambient_state() {
    iq().args(["-s", &format!("return {PROBE}")])
        .assert()
        .success()
        .stdout("os=nil random=nil randomseed=nil");
}

#[test]
fn ied_has_no_ambient_state() {
    ied()
        .args(["-s", &format!("print({PROBE})")])
        .assert()
        .success()
        .stdout(predicate::str::contains("os=nil random=nil randomseed=nil"));
}

/// Absent, not stubbed: indexing `os` is a plain nil-index error, and it is the
/// script's fault (exit 3), not a host failure.
#[test]
fn iq_calling_os_is_a_script_error() {
    iq().args(["-s", "return tostring(os.time())"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("attempt to index nil with 'time'"));
}

/// The rest of `math` survives — only the two randomness entries are removed.
#[test]
fn iq_keeps_the_rest_of_math() {
    iq().args([
        "-s",
        r#"return tostring(math.floor(3.7)) .. "," .. tostring(math.max(1, 9))"#,
    ])
    .assert()
    .success()
    .stdout("3,9");
}

/// Removing entries must not leave `math` writable: `seal` still freezes it.
#[test]
fn iq_math_table_is_still_frozen() {
    iq().args([
        "-s",
        r#"local ok = pcall(function() math.random = 1 end) return tostring(ok)"#,
    ])
    .assert()
    .success()
    .stdout("false");
}

// --- allowed in the full toolset ---

#[test]
fn ito_has_ambient_state() {
    ito()
        .args(["-s", &format!("print({PROBE})")])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "os=table random=function randomseed=function",
        ));
}

/// `os.date` and `math.random` actually work, not merely exist.
#[test]
fn ito_ambient_state_is_functional() {
    ito()
        .args([
            "-s",
            r#"math.randomseed(42)
               local n = math.random(1, 6)
               assert(n >= 1 and n <= 6, "range")
               print("ok=" .. tostring(#os.date("!%Y") == 4))"#,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("ok=true"));
}

// --- os.getenv: `ito` only ---

/// Luau's sandboxed `os` has no `getenv`; `ito` restores it, and only `ito`.
/// The name is what matters — a script portable across binaries tests
/// `if os.getenv then`, since `os` itself exists in `ito` regardless.
#[test]
fn ito_reads_the_environment() {
    ito()
        .args([
            "-s",
            r#"print("v=" .. tostring(os.getenv("ITO_TEST_VAR")))"#,
        ])
        .env("ITO_TEST_VAR", "hello")
        .assert()
        .success()
        .stdout(predicate::str::contains("v=hello"));
}

/// An unset variable reads as nil, so `or` supplies a default.
#[test]
fn ito_unset_variable_is_nil() {
    ito()
        .args([
            "-s",
            r#"print("v=" .. (os.getenv("ITO_TEST_ABSENT") or "fallback"))"#,
        ])
        .env_remove("ITO_TEST_ABSENT")
        .assert()
        .success()
        .stdout(predicate::str::contains("v=fallback"));
}

/// Set-but-empty is `""`, not nil — the two stay distinguishable, which is the
/// contract `os.getenv` has always had.
#[test]
fn ito_empty_variable_is_not_nil() {
    ito()
        .args([
            "-s",
            r#"local v = os.getenv("ITO_TEST_EMPTY")
               print("type=" .. typeof(v) .. " len=" .. tostring(#v))"#,
        ])
        .env("ITO_TEST_EMPTY", "")
        .assert()
        .success()
        .stdout(predicate::str::contains("type=string len=0"));
}

/// The sandboxed binaries have no `getenv` at all — and in their case not even
/// the `os` table to hang it on.
#[test]
fn iq_and_ied_have_no_getenv() {
    const EXPR: &str = r#""g=" .. typeof((os or {}).getenv)"#;
    // `iq` returns its output; `ied` prints it.
    for (mut cmd, src) in [
        (iq(), format!("return {EXPR}")),
        (ied(), format!("print({EXPR})")),
    ] {
        cmd.args(["-s", &src])
            .env("ITO_TEST_VAR", "hello")
            .assert()
            .success()
            .stdout(predicate::str::contains("g=nil"));
    }
}

// --- `time`: the value half everywhere, the clock half in `ito` only ---

/// The clock-reading names, which are the ambient half of `time`. `parse` is
/// among them because it reads the host's UTC offset for an input that carries
/// none — a function that consulted ambient state only for *some* inputs would
/// be worse than one that is simply absent.
const TIME_PROBE: &str = r#""now=" .. typeof(time.now)
    .. " parse=" .. typeof(time.parse)
    .. " local=" .. typeof(time.format_local)
    .. " rfc=" .. typeof(time.parse_rfc3339)
    .. " delta=" .. typeof(time.delta)"#;

/// The pure conversions are present in the deterministic binaries and the
/// clock-reading ones are not, so `if time.parse then` is a feature test.
#[test]
fn iq_and_ied_have_only_the_value_half_of_time() {
    for (mut cmd, src) in [
        (iq(), format!("return {TIME_PROBE}")),
        (ied(), format!("print({TIME_PROBE})")),
    ] {
        cmd.args(["-s", &src])
            .assert()
            .success()
            .stdout(predicate::str::contains(
                "now=nil parse=nil local=nil rfc=function delta=function",
            ));
    }
}

#[test]
fn ito_has_both_halves_of_time() {
    ito()
        .args(["-s", &format!("print({TIME_PROBE})")])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "now=function parse=function local=function \
             rfc=function delta=function",
        ));
}

/// What `iq` keeps is the whole of RFC 3339 — the offset-bearing forms need no
/// zone context, so they are pure value conversions.
#[test]
fn iq_still_converts_rfc3339_and_deltas() {
    iq().args([
        "-s",
        r#"local t = time.parse_rfc3339("2026-08-10T14:00:00+02:00")
           return time.format(t) .. " " .. tostring(time.delta("1h30m"))"#,
    ])
    .assert()
    .success()
    // The offset is absorbed into the instant: `+02:00` formats back as `Z`.
    .stdout("2026-08-10T12:00:00Z 5400");
}

/// Named zones are a **value** conversion — the zone is an argument and the
/// tzdb is compiled in — so they must survive where the clock does not. This is
/// the ambient-state line, asserted end-to-end rather than assumed.
#[test]
fn iq_and_ied_can_render_named_zones() {
    const EXPR: &str = r#"time.format_tz(1786363200, "Europe/Berlin")
        .. " " .. time.zone_offset(1736510400, "Europe/Berlin")"#;
    for (mut cmd, src) in [
        (iq(), format!("return {EXPR}")),
        (ied(), format!("print({EXPR})")),
    ] {
        cmd.args(["-s", &src])
            .assert()
            .success()
            // Summer is +02:00 and winter +01:00: the DST rules came from the
            // bundle, not from the host.
            .stdout(predicate::str::contains("2026-08-10T14:00:00+02:00 +01:00"));
    }
}

/// ...and no zone-less overload sneaks the *host's* zone in behind them.
#[test]
fn iq_cannot_ask_for_the_host_zone() {
    iq().args([
        "-s",
        r#"return "off=" .. tostring(pcall(time.zone_offset, 0))
             .. " local_zone=" .. typeof(time.local_zone)
             .. " fmt_local=" .. typeof(time.format_local)"#,
    ])
    .assert()
    .success()
    .stdout("off=false local_zone=nil fmt_local=nil");
}

/// A `time.parse` call under `iq` is a plain nil-call error, and the script's
/// fault (exit 3) — the same shape as reaching for `os` there.
#[test]
fn iq_calling_time_parse_is_a_script_error() {
    iq().args(["-s", r#"return time.parse("2026-08-10")"#])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("attempt to call a nil value"));
}

/// `os.time` and friends are untouched by the new module: `time` is additive,
/// so a script reaching for the Lua name it knows still finds it.
#[test]
fn ito_keeps_os_time_alongside_the_time_module() {
    ito()
        .args([
            "-s",
            r#"print("os=" .. typeof(os.time()) .. " time=" .. typeof(time.now())
                     .. " close=" .. tostring(math.abs(os.time() - time.now()) < 5))"#,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("os=number time=number close=true"));
}

/// Read-only: `seal` freezes `os` after `getenv` is added, so a script cannot
/// replace it — the same guarantee the rest of the stdlib has.
#[test]
fn ito_os_table_is_still_frozen() {
    ito()
        .args([
            "-s",
            r#"print(tostring(pcall(function() os.getenv = 1 end)))"#,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("false"));
}

/// There is no setter in any binary.
#[test]
fn ito_has_no_setenv() {
    ito()
        .args(["-s", r#"print("s=" .. typeof(os.setenv))"#])
        .assert()
        .success()
        .stdout(predicate::str::contains("s=nil"));
}

/// Seeding is honoured, so a script that wants reproducibility can have it.
#[test]
fn ito_seeded_random_is_reproducible() {
    let script = "math.randomseed(7) print(math.random())";
    let first = ito().args(["-s", script]).assert().success();
    let expected = String::from_utf8(first.get_output().stdout.clone()).unwrap();

    ito()
        .args(["-s", script])
        .assert()
        .success()
        .stdout(expected);
}
