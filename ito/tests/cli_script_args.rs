//! Integration tests for the script's own arguments: everything after
//! `SCRIPT` reaches it verbatim as `argv`, and the binary's flags go before it.

use assert_cmd::Command;
use assert_cmd::assert::Assert;
use predicates::prelude::*;

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

fn ito() -> Command {
    Command::cargo_bin("ito").unwrap()
}

fn ied() -> Command {
    Command::cargo_bin("ied").unwrap()
}

/// Prints `argv` as `<count> <a|b|...>`.
const ECHO: &str = r#"print(#argv .. " " .. table.concat(argv, "|"))"#;

#[test]
fn argv_carries_everything_after_the_script() {
    ito()
        .run_script(ECHO, &["--env", "prod"])
        .success()
        .stdout(predicate::str::contains("2 --env|prod"));
}

#[test]
fn argv_is_empty_without_arguments() {
    ito()
        .run_script(ECHO, &[])
        .success()
        .stdout(predicate::str::contains("0 "));
}

#[test]
fn argv_does_not_start_with_the_program_name() {
    // Unlike a C argv, element 1 is the first *given* argument.
    ito()
        .run_script(r#"print(argv[1])"#, &["first"])
        .success()
        .stdout(predicate::str::contains("first"));
}

#[test]
fn script_arguments_are_not_parsed_by_clap() {
    // `--nope` would be an unknown flag before SCRIPT, and is just a string after.
    ito()
        .run_script(ECHO, &["--nope"])
        .success()
        .stdout(predicate::str::contains("1 --nope"));
}

#[test]
fn the_binarys_own_flags_after_the_script_belong_to_the_script() {
    // None of these is intercepted: not `-v`, not an early exit like `--doc`,
    // not `-C` with its value, and not a `--` terminator.
    ito()
        .run_script(ECHO, &["-v", "--doc", "-C", "/nowhere", "--", "x"])
        .success()
        .stdout(predicate::str::contains("6 -v|--doc|-C|/nowhere|--|x"));
}

#[test]
fn a_former_separator_is_an_ordinary_argument() {
    ito()
        .run_script(ECHO, &["a", "::", "b"])
        .success()
        .stdout(predicate::str::contains("3 a|::|b"));
}

#[test]
fn flags_before_the_script_are_the_binarys() {
    // `-C` still applies when it comes first, and does not reach `argv`.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("marker.txt"), "here").unwrap();
    ito()
        .args(["-C", dir.path().to_str().unwrap()])
        .run_script(r#"print(#argv .. " " .. fs.read("marker.txt"))"#, &["a"])
        .success()
        .stdout(predicate::str::contains("1 here"));
}

#[test]
fn argv_exists_in_ied() {
    ied()
        .run_script(r#"print(argv[1])"#, &["hello"])
        .success()
        .stdout(predicate::str::contains("hello"));
}
