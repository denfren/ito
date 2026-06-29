use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn ito() -> Command {
    Command::cargo_bin("ito").unwrap()
}

#[test]
fn no_script_lists_scripts_and_exits_zero() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("deploy.lua"), r#"print("deploy")"#).unwrap();
    ito()
        .args(["-C", dir.path().to_str().unwrap()])
        .assert()
        .success()
        // Listed as the path you would type, since that is how scripts are run.
        .stdout(predicate::str::contains("ito ./deploy.lua"))
        .stdout(predicate::str::contains("ito --help"))
        .stdout(predicate::str::contains("ito --doc"));
}

#[test]
fn no_script_no_scripts_prints_hint() {
    let dir = TempDir::new().unwrap();
    ito()
        .args(["-C", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("No scripts found"))
        .stdout(predicate::str::contains("run it by path"))
        .stdout(predicate::str::contains("ito --help"))
        .stdout(predicate::str::contains("ito --doc"));
}
