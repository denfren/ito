//! The `cli` module across all four binaries.
//!
//! The unit tests in `scripting/cli.rs` cover the grammar. What needs a real
//! process is the part that made widening `cli` past `ito`/`irun` safe: what
//! `cli.parse()` prints is *terminal*, so a run that prints it never reaches
//! `emit_returned` (`iq`) or `emit_reports` (`ied`) and cannot interleave with
//! either. That is a claim about whole-process stream contents and exit codes,
//! which is only observable from outside.

use assert_cmd::Command;
use predicates::prelude::*;

fn ito() -> Command {
    Command::cargo_bin("ito").unwrap()
}

fn iq() -> Command {
    Command::cargo_bin("iq").unwrap()
}

fn ied() -> Command {
    Command::cargo_bin("ied").unwrap()
}

fn irun() -> Command {
    Command::cargo_bin("irun").unwrap()
}

/// A script declaring one option and one positional, echoing both.
const ECHO: &str = r#"
local up = cli.opt("u/upper", "shout it")
local word = cli.arg("word", "what to say")
function main()
    cli.parse()
    local w = word()
    if up() then w = w:upper() end
    print(w)
end
"#;

// ---------------------------------------------------------------------------
// Available everywhere
// ---------------------------------------------------------------------------

#[test]
fn every_binary_has_cli() {
    // Each reports through the channel it actually has: `iq` has no `print`
    // (its return value is the output), and `irun` has no flags to pass `-s`.
    ito()
        .args(["-s", "print(type(cli))"])
        .assert()
        .success()
        .stdout(predicate::str::contains("table"));

    ied()
        .args(["-s", "print(type(cli))"])
        .assert()
        .success()
        .stdout(predicate::str::contains("table"));

    iq().args(["-s", "return type(cli)"])
        .assert()
        .success()
        .stdout(predicate::eq("table"));

    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("s.luau");
    std::fs::write(&script, "print(type(cli))").unwrap();
    irun()
        .arg(&script)
        .assert()
        .success()
        .stdout(predicate::str::contains("table"));
}

#[test]
fn parses_options_and_positionals() {
    ito()
        .args(["-s", ECHO, "::", "--upper", "hello"])
        .assert()
        .success()
        .stdout(predicate::str::contains("HELLO"));
}

// ---------------------------------------------------------------------------
// `iq`: the return value owns stdout
// ---------------------------------------------------------------------------

/// A filter whose answer is its return value, as `iq` requires.
const FILTER: &str = r#"
local word = cli.arg("word", "what to return")
function main()
    cli.parse()
    return word()
end
"#;

#[test]
fn iq_writes_only_the_answer() {
    // Verbatim: no trailing newline, nothing from `cli` alongside it.
    iq().args(["-s", FILTER, "::", "hi"])
        .assert()
        .success()
        .stdout(predicate::eq("hi"));
}

#[test]
fn iq_help_does_not_reach_the_answer() {
    // Terminal: `emit_returned` never runs, so stdout is the help alone and
    // stderr stays empty -- a `--help` is an answer, not a complaint.
    iq().args(["-s", FILTER, "::", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Arguments:"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn iq_usage_error_leaves_stdout_clean() {
    // The pipe-purity guarantee: a bad command line must not put a partial or
    // spurious answer where `$(iq ...)` would capture it.
    iq().args(["-s", FILTER, "::", "--nope"])
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("unknown option \"--nope\""));
}

// ---------------------------------------------------------------------------
// `ied`: a report owns stdout
// ---------------------------------------------------------------------------

/// An edit driven by the command line.
const EDIT: &str = r#"
local suffix = cli.opt("s/suffix=", "text to append")
local target = cli.arg("target", "file to edit")
function main()
    cli.parse()
    local p = target()
    fs.write(p, fs.read(p) .. (suffix() or "!"))
end
"#;

/// A directory holding one file, plus its path as a string.
fn fixture() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "hello").unwrap();
    let path = dir.path().to_str().unwrap().to_string();
    (dir, path)
}

#[test]
fn ied_diff_reports_the_change() {
    let (dir, path) = fixture();
    ied()
        .args(["-s", EDIT, "-C", &path, "--diff", "::", "-s", "?", "f.txt"])
        .assert()
        .success()
        .stdout(predicate::str::contains("+hello?"));
    // A bare run drops the change set: nothing reached disk.
    let on_disk = std::fs::read_to_string(dir.path().join("f.txt")).unwrap();
    assert_eq!(on_disk, "hello");
}

#[test]
fn ied_help_replaces_the_report() {
    // The interleaving case that mattered: `--diff` and `--help` together must
    // print the help alone, because `emit_reports` is downstream of `eval`.
    let (dir, path) = fixture();
    ied()
        .args(["-s", EDIT, "-C", &path, "--diff", "::", "--help"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("Arguments:").and(predicate::str::contains("+hello?").not()),
        );
    let on_disk = std::fs::read_to_string(dir.path().join("f.txt")).unwrap();
    assert_eq!(on_disk, "hello");
}

#[test]
fn ied_usage_error_writes_no_report() {
    let (_dir, path) = fixture();
    ied()
        .args(["-s", EDIT, "-C", &path, "--diff", "::", "--nope"])
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty());
}

#[test]
fn ied_help_does_not_write_even_under_w() {
    // `--write` is the flag that commits the change set. Help exits before the
    // script body has run at all, so there is nothing to commit.
    let (dir, path) = fixture();
    ied()
        .args(["-s", EDIT, "-C", &path, "--write", "::", "--help"])
        .assert()
        .success();
    let on_disk = std::fs::read_to_string(dir.path().join("f.txt")).unwrap();
    assert_eq!(on_disk, "hello");
}

// ---------------------------------------------------------------------------
// Exit codes and routing, shared
// ---------------------------------------------------------------------------

#[test]
fn help_goes_to_stdout_and_exits_zero() {
    ito()
        .args(["-s", ECHO, "::", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("-u, --upper"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn usage_errors_go_to_stderr_and_exit_two() {
    ito()
        .args(["-s", ECHO, "::", "--bogus"])
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("unknown option \"--bogus\""));
}

#[test]
fn a_missing_required_argument_is_a_usage_error() {
    ito()
        .args(["-s", ECHO])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("missing required argument <word>"));
}

#[test]
fn help_beats_a_missing_required_argument() {
    // The script whose help you need is the one you cannot yet invoke.
    ito()
        .args(["-s", ECHO, "::", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Arguments:"));
}

#[test]
fn a_script_error_is_still_exit_three() {
    // The exit sentinel must not swallow ordinary failures.
    ito()
        .args(["-s", r#"cli.parse() error("boom")"#])
        .assert()
        .code(3);
}

#[test]
fn a_bad_spec_is_a_script_error_not_a_usage_error() {
    // A malformed spec is the script's bug, so it raises as an ordinary Luau
    // error (exit 3) rather than printing usage.
    ito()
        .args(["-s", r#"cli.opt("ab/cd=x")"#])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("bad option spec"));
}

#[test]
fn usage_line_names_no_program() {
    // Under a shebang the binary's argv[0] is the interpreter, not the script,
    // so no name is printed at all.
    ito()
        .args(["-s", ECHO, "::", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ito").not());
}

// ---------------------------------------------------------------------------
// `irun`: no separator
// ---------------------------------------------------------------------------

#[test]
fn irun_needs_no_separator() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("s.luau");
    std::fs::write(&script, ECHO).unwrap();
    irun()
        .args([script.to_str().unwrap(), "--upper", "hey"])
        .assert()
        .success()
        .stdout(predicate::str::contains("HEY"));
}

#[test]
fn irun_help_is_the_scripts_own() {
    // `irun` has no flags, so `--help` can only ever be the script's.
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("s.luau");
    std::fs::write(&script, ECHO).unwrap();
    irun()
        .args([script.to_str().unwrap(), "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("what to say"));
}
