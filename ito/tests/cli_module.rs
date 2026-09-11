//! The `cli` module across both binaries.
//!
//! The unit tests in `scripting/cli.rs` cover the grammar. What needs a real
//! process is the part that made widening `cli` past `ito` safe: what
//! `cli.parse()` prints is *terminal*, so a run that prints it never reaches
//! `emit_reports` (`ied`) and cannot interleave with it. That is a claim about
//! whole-process stream contents and exit codes, which is only observable from
//! outside.

use assert_cmd::Command;
use assert_cmd::assert::Assert;
use predicates::prelude::*;

fn ito() -> Command {
    Command::cargo_bin("ito").unwrap()
}

fn ied() -> Command {
    Command::cargo_bin("ied").unwrap()
}

/// Runs a script given as source, followed by `args`. A script is always a
/// path, so the source is written to a temporary file first.
trait RunScript {
    fn run_script(&mut self, src: &str, args: &[&str]) -> Assert;
}

impl RunScript for Command {
    fn run_script(&mut self, src: &str, args: &[&str]) -> Assert {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("s.luau");
        std::fs::write(&script, src).unwrap();
        self.arg(&script).args(args).assert()
    }
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
    ito()
        .run_script("print(type(cli))", &[])
        .success()
        .stdout(predicate::str::contains("table"));

    ied()
        .run_script("print(type(cli))", &[])
        .success()
        .stdout(predicate::str::contains("table"));
}

#[test]
fn parses_options_and_positionals() {
    ito()
        .run_script(ECHO, &["--upper", "hello"])
        .success()
        .stdout(predicate::str::contains("HELLO"));
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
        .args(["-C", &path, "--diff"])
        .run_script(EDIT, &["-s", "?", "f.txt"])
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
        .args(["-C", &path, "--diff"])
        .run_script(EDIT, &["--help"])
        .success()
        .stdout(
            predicate::str::contains("Usage:").and(predicate::str::contains("+hello?").not()),
        );
    let on_disk = std::fs::read_to_string(dir.path().join("f.txt")).unwrap();
    assert_eq!(on_disk, "hello");
}

#[test]
fn ied_usage_error_writes_no_report() {
    let (_dir, path) = fixture();
    ied()
        .args(["-C", &path, "--diff"])
        .run_script(EDIT, &["--nope"])
        .code(2)
        .stdout(predicate::str::is_empty());
}

#[test]
fn ied_help_does_not_write_even_under_w() {
    // `--write` is the flag that commits the change set. Help exits before the
    // script body has run at all, so there is nothing to commit.
    let (dir, path) = fixture();
    ied()
        .args(["-C", &path, "--write"])
        .run_script(EDIT, &["--help"])
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
        .run_script(ECHO, &["--help"])
        .success()
        .stdout(predicate::str::contains("-u, --upper"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn usage_errors_go_to_stderr_and_exit_two() {
    ito()
        .run_script(ECHO, &["--bogus"])
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("unknown option \"--bogus\""));
}

#[test]
fn a_missing_required_argument_is_a_usage_error() {
    ito()
        .run_script(ECHO, &[])
        .code(2)
        .stderr(predicate::str::contains("missing required argument <word>"));
}

#[test]
fn help_beats_a_missing_required_argument() {
    // The script whose help you need is the one you cannot yet invoke.
    ito()
        .run_script(ECHO, &["--help"])
        .success()
        .stdout(predicate::str::contains("Usage:"));
}

#[test]
fn a_script_error_is_still_exit_three() {
    // The exit sentinel must not swallow ordinary failures.
    ito()
        .run_script(r#"cli.parse() error("boom")"#, &[])
        .code(3);
}

#[test]
fn a_bad_spec_is_a_script_error_not_a_usage_error() {
    // A malformed spec is the script's bug, so it raises as an ordinary Luau
    // error (exit 3) rather than printing usage.
    ito()
        .run_script(r#"cli.opt("ab/cd=x")"#, &[])
        .code(3)
        .stderr(predicate::str::contains("bad option spec"));
}

#[test]
fn usage_line_names_no_program() {
    // Under a shebang the binary's argv[0] is the interpreter, not the script,
    // so no name is printed at all.
    ito()
        .run_script(ECHO, &["--help"])
        .success()
        .stdout(predicate::str::contains("ito").not());
}

// ---------------------------------------------------------------------------
// After SCRIPT, every argument is the script's
// ---------------------------------------------------------------------------

#[test]
fn help_after_the_script_is_the_scripts_own() {
    // `ito --help` is clap's; `ito s.luau --help` never reaches clap.
    ito()
        .run_script(ECHO, &["--help"])
        .success()
        .stdout(predicate::str::contains("what to say"))
        .stdout(predicate::str::contains("full ito standard library").not());
}

#[test]
fn binary_flag_spellings_reach_cli_parse() {
    // `-v` is the binary's verbosity flag before SCRIPT; after it, `cli.parse`
    // sees it like any other argument.
    ito()
        .run_script(
            r#"local v = cli.opt("v/verbose", "chatty")
               function main() cli.parse() print("v=" .. tostring(v())) end"#,
            &["-v"],
        )
        .success()
        .stdout(predicate::str::contains("v=true"));
}
