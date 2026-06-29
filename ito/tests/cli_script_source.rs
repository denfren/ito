//! Integration tests for how the CLI decides *what* to run and *where* paths
//! point: `SCRIPT` as a path, `-C` applying after path arguments, and `-r`
//! granting imports to scripts that have no file of their own.

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

// --- `-C` is accepted alongside every script source ---

#[test]
fn dash_c_works_with_inline() {
    let dir = TempDir::new().unwrap();
    ito()
        .args(["-C", dir.path().to_str().unwrap(), "-s", r#"print("ok")"#])
        .assert()
        .success()
        .stdout(predicate::str::contains("ok"));
}

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

// --- `-r` grants imports to stdin / `-s` scripts ---

/// A tmpdir holding `lib/mod.luau`, importable as `./lib/mod`.
fn lib_fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("lib")).unwrap();
    fs::write(
        dir.path().join("lib/mod.luau"),
        r#"return { id = function() return "LIB" end }"#,
    )
    .unwrap();
    dir
}

#[test]
fn stdin_cannot_require_without_dash_r() {
    let dir = lib_fixture();
    ito()
        .current_dir(dir.path())
        .arg("-")
        .write_stdin(r#"require("./lib/mod")"#)
        .assert()
        .failure()
        .stderr(predicate::str::contains("imports are disabled"))
        .stderr(predicate::str::contains("-r"));
}

#[test]
fn inline_cannot_require_without_dash_r() {
    let dir = lib_fixture();
    ito()
        .current_dir(dir.path())
        .args(["-s", r#"require("./lib/mod")"#])
        .assert()
        .failure()
        .stderr(predicate::str::contains("imports are disabled"));
}

#[test]
fn dash_r_allows_stdin_to_require() {
    let dir = lib_fixture();
    ito()
        .current_dir(dir.path())
        .args(["-", "-r", "."])
        .write_stdin(r#"print(require("./lib/mod").id())"#)
        .assert()
        .success()
        .stdout(predicate::str::contains("LIB"));
}

#[test]
fn dash_r_allows_inline_to_require() {
    let dir = lib_fixture();
    ito()
        .current_dir(dir.path())
        .args(["-s", r#"print(require("./lib/mod").id())"#, "-r", "."])
        .assert()
        .success()
        .stdout(predicate::str::contains("LIB"));
}

#[test]
fn dash_r_resolves_against_the_invocation_dir_not_dash_c() {
    // `-C` applies after path arguments, so `-r .` means the directory the
    // user typed it in even though `-C` moves the `fs` root elsewhere.
    let dir = lib_fixture();
    fs::create_dir(dir.path().join("other")).unwrap();
    ito()
        .current_dir(dir.path())
        .args(["-", "-r", ".", "-C", "other"])
        .write_stdin(r#"print(require("./lib/mod").id())"#)
        .assert()
        .success()
        .stdout(predicate::str::contains("LIB"));
}

#[test]
fn dash_r_confines_imports_to_its_tree() {
    let dir = lib_fixture();
    fs::write(
        dir.path().join("secret.luau"),
        r#"return { id = function() return "ESCAPED" end }"#,
    )
    .unwrap();
    // Rooted at `lib`, a `../` reaches out of the tree and is refused.
    ito()
        .current_dir(dir.path())
        .args(["-", "-r", "lib"])
        .write_stdin(r#"print(require("../secret").id())"#)
        .assert()
        .failure()
        .stdout(predicate::str::contains("ESCAPED").not());
}

#[test]
fn dash_r_is_rejected_with_a_script_file() {
    let dir = lib_fixture();
    fs::write(dir.path().join("main.lua"), r#"print("hi")"#).unwrap();
    // A file already imports relative to itself, so `-r` would be ambiguous.
    ito()
        .current_dir(dir.path())
        .args(["main.lua", "-r", "."])
        .assert()
        .failure()
        .stderr(predicate::str::contains("-r/--require applies to stdin"));
}
