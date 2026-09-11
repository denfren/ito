//! Integration tests for how the CLI decides *what* to run and *where* paths
//! point: `SCRIPT` as a path (the only script source), and `-C` applying
//! after path arguments.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn ito() -> Command {
    Command::cargo_bin("ito").unwrap()
}

/// A tmpdir with an outer and an inner `marker.txt`, plus a script that reads
/// `marker.txt` through `fs` — so the output reveals which directory the `fs`
/// root ended up at.
fn nested_fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("marker.txt"), "outer").unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("sub/marker.txt"), "inner").unwrap();
    fs::write(
        dir.path().join("s.lua"),
        r#"print("marker=" .. tostring(fs.read("marker.txt")))"#,
    )
    .unwrap();
    dir
}

// --- `-C` is accepted before the script ---

#[test]
fn dash_c_works_with_a_path() {
    let dir = TempDir::new().unwrap();
    let script = dir.path().join("s.lua");
    fs::write(&script, r#"print("run")"#).unwrap();
    ito()
        .args(["-C", dir.path().to_str().unwrap(), script.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("run"));
}

// --- `SCRIPT` is a path, resolved before `-C` takes effect ---

#[test]
fn relative_path_resolves_against_the_invocation_dir() {
    let dir = nested_fixture();
    ito()
        .current_dir(dir.path())
        .args(["-C", "sub", "./s.lua"])
        .assert()
        .success()
        // Script found via the invocation dir, but `fs` is rooted at `sub`.
        .stdout(predicate::str::contains("marker=inner"));
}

#[test]
fn bare_relative_path_resolves_against_the_invocation_dir() {
    // No `./` needed: a bare argument is a path like any other, so this finds
    // the script beside the user even though `-C` points elsewhere.
    let dir = nested_fixture();
    ito()
        .current_dir(dir.path())
        .args(["-C", "sub", "s.lua"])
        .assert()
        .success()
        .stdout(predicate::str::contains("marker=inner"));
}

#[test]
fn parent_relative_path_resolves_against_the_invocation_dir() {
    let dir = nested_fixture();
    ito()
        .current_dir(dir.path().join("sub"))
        .args(["-C", ".", "../s.lua"])
        .assert()
        .success()
        .stdout(predicate::str::contains("marker=inner"));
}

#[test]
fn absolute_path_is_unaffected_by_dash_c() {
    let dir = nested_fixture();
    let script = dir.path().join("s.lua");
    ito()
        .current_dir(dir.path())
        .args(["-C", "sub", script.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("marker=inner"));
}

#[test]
fn a_name_that_is_not_a_path_is_not_found() {
    // There is no name lookup: `deploy` is a path, and no such file exists.
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("deploy.lua"), r#"print("deploy")"#).unwrap();
    ito()
        .current_dir(dir.path())
        .arg("deploy")
        .assert()
        .failure()
        .stderr(predicate::str::contains("no script at"));
}

#[test]
fn missing_script_exits_nonzero() {
    let dir = TempDir::new().unwrap();
    ito()
        .current_dir(dir.path())
        .arg("./nope.lua")
        .assert()
        .failure()
        .stderr(predicate::str::contains("no script at"));
}

// --- bare `ito` lists what it finds ---

#[test]
fn bare_invocation_lists_scripts_as_paths() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("deploy.lua"), "").unwrap();
    fs::create_dir(dir.path().join("ito")).unwrap();
    fs::write(dir.path().join("ito/build.luau"), "").unwrap();
    // Hidden files are not offered.
    fs::write(dir.path().join(".secret.lua"), "").unwrap();

    ito()
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("ito ./deploy.lua"))
        .stdout(predicate::str::contains("ito ito/build.luau"))
        .stdout(predicate::str::contains("secret").not());
}

// --- `-` is a path like any other ---

#[test]
fn dash_is_a_path_not_stdin() {
    // There is no stdin script: `-` names a file, and there is none here.
    let dir = TempDir::new().unwrap();
    ito()
        .current_dir(dir.path())
        .arg("-")
        .write_stdin(r#"print("from stdin")"#)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("from stdin").not())
        .stderr(predicate::str::contains("no script at"));
}
