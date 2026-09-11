//! Integration tests for the ambient-state split: `os` and `math.random` are
//! absent in `ied` and present in `ito`, and `os.getenv` — which Luau's
//! sandboxed `os` does not ship at all — is added back for `ito` alone.
//!
//! The point is not that a call fails but that the *name* is missing, so
//! `if os then` is a usable feature test — the same contract `fs.write` has.
//! `ito` keeps both: it already spawns processes and opens sockets, so
//! withholding the clock would be a pretence. `getenv` is the one exception to
//! the table-level test, being a key that varies within a table `ito` has
//! either way, so its own test is `if os.getenv then`.

use assert_cmd::Command;
use assert_cmd::assert::Assert;
use predicates::prelude::*;

/// Runs a script given as source. A script is always a path, so the source is
/// written to a temporary file first; the file outlives the run.
trait RunScript {
    fn run_script(&mut self, src: &str) -> Assert;
}

impl RunScript for Command {
    fn run_script(&mut self, src: &str) -> Assert {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("s.luau");
        std::fs::write(&script, src).unwrap();
        self.arg(&script).assert()
    }
}

fn ied() -> Command {
    Command::cargo_bin("ied").unwrap()
}

fn ito() -> Command {
    Command::cargo_bin("ito").unwrap()
}

/// The three names as present/absent, printed by the script.
const PROBE: &str = r#""os=" .. typeof(os)
    .. " random=" .. typeof(math.random)
    .. " randomseed=" .. typeof(math.randomseed)"#;

// --- denied in the deterministic binary ---

#[test]
fn ied_has_no_ambient_state() {
    ied()
        .run_script(&format!("print({PROBE})"))
        .success()
        .stdout(predicate::str::contains("os=nil random=nil randomseed=nil"));
}

/// Absent, not stubbed: indexing `os` is a plain nil-index error, and it is the
/// script's fault (exit 3), not a host failure.
#[test]
fn ied_calling_os_is_a_script_error() {
    ied().run_script("print(tostring(os.time()))")
        .code(3)
        .stderr(predicate::str::contains("attempt to index nil with 'time'"));
}

/// The rest of `math` survives — only the two randomness entries are removed.
#[test]
fn ied_keeps_the_rest_of_math() {
    ied()
        .run_script(r#"print(tostring(math.floor(3.7)) .. "," .. tostring(math.max(1, 9)))"#)
        .success()
        .stdout("3,9\n");
}

/// Removing entries must not leave `math` writable: `seal` still freezes it.
#[test]
fn ied_math_table_is_still_frozen() {
    ied()
        .run_script(r#"local ok = pcall(function() math.random = 1 end) print(tostring(ok))"#)
        .success()
        .stdout("false\n");
}

// --- allowed in the full toolset ---

#[test]
fn ito_has_ambient_state() {
    ito()
        .run_script(&format!("print({PROBE})"))
        .success()
        .stdout(predicate::str::contains(
            "os=table random=function randomseed=function",
        ));
}

/// `os.date` and `math.random` actually work, not merely exist.
#[test]
fn ito_ambient_state_is_functional() {
    ito()
        .run_script(
            r#"math.randomseed(42)
               local n = math.random(1, 6)
               assert(n >= 1 and n <= 6, "range")
               print("ok=" .. tostring(#os.date("!%Y") == 4))"#,
        )
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
        .env("ITO_TEST_VAR", "hello")
        .run_script(r#"print("v=" .. tostring(os.getenv("ITO_TEST_VAR")))"#)
        .success()
        .stdout(predicate::str::contains("v=hello"));
}

/// An unset variable reads as nil, so `or` supplies a default.
#[test]
fn ito_unset_variable_is_nil() {
    ito()
        .env_remove("ITO_TEST_ABSENT")
        .run_script(r#"print("v=" .. (os.getenv("ITO_TEST_ABSENT") or "fallback"))"#)
        .success()
        .stdout(predicate::str::contains("v=fallback"));
}

/// Set-but-empty is `""`, not nil — the two stay distinguishable, which is the
/// contract `os.getenv` has always had.
#[test]
fn ito_empty_variable_is_not_nil() {
    ito()
        .env("ITO_TEST_EMPTY", "")
        .run_script(
            r#"local v = os.getenv("ITO_TEST_EMPTY")
               print("type=" .. typeof(v) .. " len=" .. tostring(#v))"#,
        )
        .success()
        .stdout(predicate::str::contains("type=string len=0"));
}

/// The sandboxed binary has no `getenv` at all — and not even the `os` table to
/// hang it on.
#[test]
fn ied_has_no_getenv() {
    const EXPR: &str = r#""g=" .. typeof((os or {}).getenv)"#;
    ied()
        .env("ITO_TEST_VAR", "hello")
        .run_script(&format!("print({EXPR})"))
        .success()
        .stdout(predicate::str::contains("g=nil"));
}

// --- `time`: the value half everywhere, the clock half in `ito` only ---

/// The clock-reading names, which are the ambient half of `time`.
/// `parse_civil_local` is among them because it reads the host's UTC offset
/// for a value that carries none — a function that consulted ambient state
/// only for *some* inputs would be worse than one that is simply absent.
const TIME_PROBE: &str = r#""now=" .. typeof(time.now)
    .. " civil_local=" .. typeof(time.parse_civil_local)
    .. " local=" .. typeof(time.format_rfc3339_local)
    .. " rfc=" .. typeof(time.parse_rfc3339)
    .. " delta=" .. typeof(time.parse_delta)"#;

/// The pure conversions are present in the deterministic binary and the
/// clock-reading ones are not, so `if time.parse_civil_local then` is a
/// feature test.
#[test]
fn ied_has_only_the_value_half_of_time() {
    ied()
        .run_script(&format!("print({TIME_PROBE})"))
        .success()
        .stdout(predicate::str::contains(
            "now=nil civil_local=nil local=nil rfc=function delta=function",
        ));
}

#[test]
fn ito_has_both_halves_of_time() {
    ito()
        .run_script(&format!("print({TIME_PROBE})"))
        .success()
        .stdout(predicate::str::contains(
            "now=function civil_local=function local=function \
             rfc=function delta=function",
        ));
}

/// What `ied` keeps is the whole of RFC 3339 — the offset-bearing forms need no
/// zone context, so they are pure value conversions.
#[test]
fn ied_still_converts_rfc3339_and_deltas() {
    ied()
        .run_script(
            r#"local t = time.parse_rfc3339("2026-08-10T14:00:00+02:00")
               print(time.format_rfc3339_utc(t) .. " " .. tostring(time.parse_delta("1h30m")))"#,
        )
        .success()
        // The offset is absorbed into the instant: `+02:00` formats back as `Z`.
        .stdout("2026-08-10T12:00:00Z 5400\n");
}

/// Named zones are a **value** conversion — the zone is an argument and the
/// tzdb is compiled in — so they must survive where the clock does not. This is
/// the ambient-state line, asserted end-to-end rather than assumed.
#[test]
fn ied_can_render_named_zones() {
    const EXPR: &str = r#"time.format_rfc3339_tz(1786363200, "Europe/Berlin")
        .. " " .. time.format_fmt_tz(1736510400, "%:z", "Europe/Berlin")"#;
    ied()
        .run_script(&format!("print({EXPR})"))
        .success()
        // Summer is +02:00 and winter +01:00: the DST rules came from the
        // bundle, not from the host.
        .stdout(predicate::str::contains("2026-08-10T14:00:00+02:00 +01:00"));
}

/// ...and no zone-less overload sneaks the *host's* zone in behind them.
#[test]
fn ied_cannot_ask_for_the_host_zone() {
    ied()
        .run_script(
            r#"print("civil_local=" .. typeof(time.parse_civil_local)
                 .. " fmt_local=" .. typeof(time.format_fmt_local)
                 .. " rfc3339_local=" .. typeof(time.format_rfc3339_local))"#,
        )
        .success()
        .stdout("civil_local=nil fmt_local=nil rfc3339_local=nil\n");
}

/// A `time.parse_civil_local` call under `ied` is a plain nil-call error, and
/// the script's fault (exit 3) — the same shape as reaching for `os` there.
#[test]
fn ied_calling_time_parse_civil_local_is_a_script_error() {
    ied()
        .run_script(r#"print(time.parse_civil_local("2026-08-10"))"#)
        .code(3)
        .stderr(predicate::str::contains("attempt to call a nil value"));
}

/// `os.time` and friends are untouched by the new module: `time` is additive,
/// so a script reaching for the Lua name it knows still finds it.
#[test]
fn ito_keeps_os_time_alongside_the_time_module() {
    ito()
        .run_script(
            r#"print("os=" .. typeof(os.time()) .. " time=" .. typeof(time.now())
                     .. " close=" .. tostring(math.abs(os.time() - time.now()) < 5))"#,
        )
        .success()
        .stdout(predicate::str::contains("os=number time=number close=true"));
}

/// Read-only: `seal` freezes `os` after `getenv` is added, so a script cannot
/// replace it — the same guarantee the rest of the stdlib has.
#[test]
fn ito_os_table_is_still_frozen() {
    ito()
        .run_script(r#"print(tostring(pcall(function() os.getenv = 1 end)))"#)
        .success()
        .stdout(predicate::str::contains("false"));
}

/// There is no setter in any binary.
#[test]
fn ito_has_no_setenv() {
    ito()
        .run_script(r#"print("s=" .. typeof(os.setenv))"#)
        .success()
        .stdout(predicate::str::contains("s=nil"));
}

/// Seeding is honoured, so a script that wants reproducibility can have it.
#[test]
fn ito_seeded_random_is_reproducible() {
    let script = "math.randomseed(7) print(math.random())";
    let first = ito().run_script(script).success();
    let expected = String::from_utf8(first.get_output().stdout.clone()).unwrap();

    ito().run_script(script).success().stdout(expected);
}
