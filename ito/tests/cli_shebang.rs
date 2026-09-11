use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn ito() -> Command {
    Command::cargo_bin("ito").unwrap()
}

#[test]
fn shebang_line_is_stripped_before_execution() {
    let dir = TempDir::new().unwrap();
    let script = dir.path().join("s.lua");
    fs::write(&script, "#!/usr/bin/env ito\nprint(\"ok\")\n").unwrap();
    ito()
        .args(["-C", dir.path().to_str().unwrap(), script.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("ok"));
}

#[test]
fn shebang_stripping_preserves_line_numbers_in_errors() {
    let dir = TempDir::new().unwrap();
    let script = dir.path().join("s.lua");
    fs::write(
        &script,
        "#!/usr/bin/env ito\nprint(\"ok\")\nerror(\"boom\")\n",
    )
    .unwrap();
    ito()
        .args(["-C", dir.path().to_str().unwrap(), script.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains(":3: boom"));
}

#[test]
fn non_shebang_first_line_starting_with_hash_bang_is_untouched_when_absent() {
    // Sanity: a script with no shebang runs unaffected (first line is code).
    let dir = TempDir::new().unwrap();
    let script = dir.path().join("s.lua");
    fs::write(&script, "print(\"no shebang\")\n").unwrap();
    ito()
        .args(["-C", dir.path().to_str().unwrap(), script.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("no shebang"));
}

#[test]
fn shebang_stripped_for_ied_too() {
    let dir = TempDir::new().unwrap();
    let edit = dir.path().join("e.luau");
    fs::write(&edit, "#!/usr/bin/env ied\nprint(\"edited\")\n").unwrap();
    Command::cargo_bin("ied")
        .unwrap()
        .arg(&edit)
        .assert()
        .success()
        .stdout(predicate::str::contains("edited"));
}

/// Writes an executable `s.ito` into `dir` whose shebang runs the built `ito`
/// with `flags`, followed by `body`.
#[cfg(unix)]
fn shebang_script(dir: &std::path::Path, flags: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    let bin = assert_cmd::cargo::cargo_bin("ito");
    let script = dir.join("s.ito");
    fs::write(
        &script,
        format!("#!/usr/bin/env -S {} {flags}\n{body}", bin.display()),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
#[cfg(unix)]
fn executed_shebang_script_reports_the_right_line() {
    // The shebang line counts as line 1, so the error is on line 3.
    let dir = TempDir::new().unwrap();
    shebang_script(dir.path(), "", "print(\"ok\")\nerror(\"boom\")\n");
    Command::new("./s.ito")
        .current_dir(dir.path())
        .assert()
        .code(3)
        .stderr(predicate::str::contains(":3: boom"));
}

#[test]
#[cfg(unix)]
fn executed_shebang_script_owns_its_arguments() {
    // What the user types after the script lands in `argv`, flag-looking or not.
    let dir = TempDir::new().unwrap();
    shebang_script(dir.path(), "", "print(table.concat(argv, \"|\"))\n");
    Command::new("./s.ito")
        .current_dir(dir.path())
        .args(["-v", "--doc", "x"])
        .assert()
        .success()
        .stdout(predicate::str::contains("-v|--doc|x"));
}

#[test]
#[cfg(unix)]
fn shebang_script_with_dash_c_is_found_and_reroots_fs() {
    // The end-to-end shebang case: the kernel hands us `./s.ito` (relative to
    // the invocation cwd) while the shebang's own `-C sub` moves the fs root.
    // Resolving the script after the chdir used to make this "no script found".
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("marker.txt"), "outer").unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("sub/marker.txt"), "inner").unwrap();
    shebang_script(
        dir.path(),
        "-C sub",
        "print(\"marker=\" .. tostring(fs.read(\"marker.txt\")))\n",
    );

    Command::new("./s.ito")
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("marker=inner"));
}
