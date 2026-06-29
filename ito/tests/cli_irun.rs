//! `irun`: the script owns the command line.
//!
//! The property under test throughout is that `irun` claims exactly one
//! argument — the script path — and interprets nothing else. The interesting
//! cases are therefore the arguments that *would* be flags anywhere else.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn irun() -> Command {
    Command::cargo_bin("irun").unwrap()
}

/// A script echoing its own arguments, comma-joined, as `<n> <a,b,c>`.
fn echo_argv_script(dir: &TempDir) -> String {
    let script = dir.path().join("s.luau");
    fs::write(&script, r#"print(#argv .. " " .. table.concat(argv, ","))"#).unwrap();
    script.to_str().unwrap().to_string()
}

#[test]
fn argv_carries_every_argument_after_the_script() {
    let dir = TempDir::new().unwrap();
    irun()
        .args([&echo_argv_script(&dir), "--env", "prod"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 --env,prod"));
}

#[test]
fn argv_is_empty_when_only_a_script_is_given() {
    let dir = TempDir::new().unwrap();
    irun()
        .arg(echo_argv_script(&dir))
        .assert()
        .success()
        .stdout(predicate::str::contains("0 "));
}

#[test]
fn itos_own_flags_reach_the_script_untouched() {
    // The whole reason `irun` exists: `-C`, `-v` and `-s` are the script's
    // strings here, not ours. If any of these were ever intercepted, a script
    // could not define that spelling.
    let dir = TempDir::new().unwrap();
    irun()
        .args([&echo_argv_script(&dir), "-C", "/tmp", "-v", "-s"])
        .assert()
        .success()
        .stdout(predicate::str::contains("4 -C,/tmp,-v,-s"));
}

#[test]
fn doc_flag_is_not_a_flag() {
    // `--doc` after the script is just an argument; there is no help to print.
    let dir = TempDir::new().unwrap();
    irun()
        .args([&echo_argv_script(&dir), "--doc", "fs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 --doc,fs"));
}

#[test]
fn separator_is_not_special() {
    // `::` is `ito`'s delimiter because `ito` has arguments to end. `irun` has
    // none, so the token is ordinary.
    let dir = TempDir::new().unwrap();
    irun()
        .args([&echo_argv_script(&dir), "::", "a"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 ::,a"));
}

#[test]
fn a_bare_invocation_is_a_usage_error() {
    // Not the script listing the other binaries print: whoever runs `irun` has
    // one specific file in mind, it being the shebang line of one.
    irun()
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("usage:"));
}

#[test]
fn a_missing_script_fails_without_running_anything() {
    let dir = TempDir::new().unwrap();
    irun()
        .arg(dir.path().join("nope.luau"))
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("no script at"));
}

#[test]
fn the_shebang_line_is_stripped() {
    let dir = TempDir::new().unwrap();
    let script = dir.path().join("s.luau");
    fs::write(&script, "#!/usr/bin/env -S irun\nprint(\"ok\")\n").unwrap();
    irun()
        .arg(&script)
        .assert()
        .success()
        .stdout(predicate::str::contains("ok"));
}

#[test]
fn the_full_host_toolset_is_present() {
    // Same capability as `ito`: direct writes, subprocesses, env, tasks.
    let dir = TempDir::new().unwrap();
    let script = dir.path().join("s.luau");
    fs::write(
        &script,
        r#"print(fs.write ~= nil, fs.mkdir ~= nil, fs.cwd ~= nil, proc ~= nil,
                 ssh ~= nil, task ~= nil, aws ~= nil, os.getenv ~= nil)"#,
    )
    .unwrap();
    irun()
        .arg(&script)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "true\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue",
        ));
}

#[test]
fn writes_land_immediately() {
    let dir = TempDir::new().unwrap();
    let script = dir.path().join("s.luau");
    let target = dir.path().join("out.txt");
    fs::write(
        &script,
        format!(
            r#"fs.write({p:?}, "written")
               print(fs.read({p:?}))"#,
            p = target.to_str().unwrap()
        ),
    )
    .unwrap();
    irun()
        .arg(&script)
        .assert()
        .success()
        .stdout(predicate::str::contains("written"));
    assert_eq!(fs::read_to_string(&target).unwrap(), "written");
}

#[test]
fn main_is_called_after_the_body() {
    let dir = TempDir::new().unwrap();
    let script = dir.path().join("s.luau");
    fs::write(
        &script,
        "print(\"body\")\nfunction main() print(\"main \" .. argv[1]) end\n",
    )
    .unwrap();
    irun()
        .args([script.to_str().unwrap(), "prod"])
        .assert()
        .success()
        .stdout(predicate::str::contains("body"))
        .stdout(predicate::str::contains("main prod"));
}

#[test]
fn a_script_error_exits_three() {
    let dir = TempDir::new().unwrap();
    let script = dir.path().join("s.luau");
    fs::write(&script, "error(\"boom\")\n").unwrap();
    irun()
        .arg(&script)
        .assert()
        .failure()
        .code(3)
        .stderr(predicate::str::contains("boom"));
}

#[test]
fn relative_script_paths_resolve_against_the_invocation_directory() {
    // What a shebang exec relies on: the kernel hands over `./<script>`.
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("s.luau"), r#"print("relative")"#).unwrap();
    irun()
        .current_dir(dir.path())
        .arg("./s.luau")
        .assert()
        .success()
        .stdout(predicate::str::contains("relative"));
}
