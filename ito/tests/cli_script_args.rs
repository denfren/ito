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

#[test]
fn argv_carries_everything_after_the_separator() {
    ito()
        .args([
            "-s",
            r#"print(#argv .. " " .. table.concat(argv, ","))"#,
            "::",
            "--env",
            "prod",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 --env,prod"));
}

#[test]
fn argv_is_empty_without_a_separator() {
    ito()
        .args(["-s", r#"print(#argv)"#])
        .assert()
        .success()
        .stdout(predicate::str::contains("0"));
}

#[test]
fn argv_does_not_start_with_the_program_name() {
    // Unlike a C argv, element 1 is the first *given* argument.
    ito()
        .args(["-s", r#"print(argv[1])"#, "::", "first"])
        .assert()
        .success()
        .stdout(predicate::str::contains("first"));
}

#[test]
fn script_arguments_are_not_parsed_by_clap() {
    // `--nope` would be an unknown flag before `::`, and is just a string after.
    ito()
        .args(["-s", r#"print(argv[1])"#, "::", "--nope"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--nope"));
}

#[test]
fn later_separators_are_ordinary_arguments() {
    ito()
        .args([
            "-s",
            r#"print(table.concat(argv, "|"))"#,
            "::",
            "a",
            "::",
            "b",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("a|::|b"));
}

#[test]
fn argv_exists_in_iq() {
    iq().args(["-s", r#"return argv[1]"#, "::", "hello"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello"));
}

#[test]
fn argv_exists_in_ied() {
    ied()
        .args(["-s", r#"print(argv[1])"#, "::", "hello"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello"));
}
