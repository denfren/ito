use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn ito() -> Command {
    Command::cargo_bin("ito").unwrap()
}

/// A tmpdir pre-populated with one script per candidate location.
///
/// Layout:
///   deploy.ito.rhai          ← flat, dotless
///   .hidden.ito.rhai         ← flat, dotted
///   ito/subdir.rhai          ← ito/ subdir
///   .ito/dotsubdir.rhai      ← .ito/ subdir
struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        fs::write(root.join("deploy.ito.rhai"), r#"print("deploy")"#).unwrap();
        fs::write(root.join(".hidden.ito.rhai"), r#"print("hidden")"#).unwrap();

        fs::create_dir(root.join("ito")).unwrap();
        fs::write(root.join("ito/subdir.rhai"), r#"print("subdir")"#).unwrap();
        // Two candidates for 'ambiguous' — triggers the ambiguity error.
        fs::write(root.join("ambiguous.ito.rhai"), r#"print("a1")"#).unwrap();
        fs::write(root.join("ito/ambiguous.rhai"), r#"print("a2")"#).unwrap();

        fs::create_dir(root.join(".ito")).unwrap();
        fs::write(root.join(".ito/dotsubdir.rhai"), r#"print("dotsubdir")"#).unwrap();

        Self { dir }
    }

    fn path(&self) -> &std::path::Path {
        self.dir.path()
    }
}

#[test]
fn no_subcommand_lists_scripts_and_exits_zero() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("deploy.ito.rhai"), r#"print("deploy")"#).unwrap();
    ito()
        .args(["-C", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("ito deploy"))
        .stdout(predicate::str::contains("ito --help"))
        .stdout(predicate::str::contains("ito help"));
}

#[test]
fn no_subcommand_no_scripts_prints_hint() {
    let dir = TempDir::new().unwrap();
    ito()
        .args(["-C", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("No scripts found"))
        .stdout(predicate::str::contains("<name>.ito.rhai"))
        .stdout(predicate::str::contains("ito --help"))
        .stdout(predicate::str::contains("ito help"));
}

// --- -C is accepted before any subcommand ---

#[test]
fn global_dash_c_works_with_exec() {
    let dir = TempDir::new().unwrap();
    ito()
        .args(["-C", dir.path().to_str().unwrap(), "exec", r#"print("ok")"#])
        .assert()
        .success()
        .stdout(predicate::str::contains("ok"));
}

#[test]
fn global_dash_c_works_with_run() {
    let dir = TempDir::new().unwrap();
    let script = dir.path().join("s.rhai");
    fs::write(&script, r#"print("run")"#).unwrap();
    ito()
        .args([
            "-C",
            dir.path().to_str().unwrap(),
            "run",
            script.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("run"));
}

// --- named discovery via `ito <name>` ---

fn in_fixture(fx: &Fixture) -> Command {
    let mut cmd = ito();
    cmd.args(["-C", fx.path().to_str().unwrap()]);
    cmd
}

#[test]
fn discover_flat_dotless() {
    let fx = Fixture::new();
    in_fixture(&fx)
        .arg("deploy")
        .assert()
        .success()
        .stdout(predicate::str::contains("deploy"));
}

#[test]
fn discover_flat_dotted() {
    let fx = Fixture::new();
    in_fixture(&fx)
        .arg("hidden")
        .assert()
        .success()
        .stdout(predicate::str::contains("hidden"));
}

#[test]
fn discover_ito_subdir() {
    let fx = Fixture::new();
    in_fixture(&fx)
        .arg("subdir")
        .assert()
        .success()
        .stdout(predicate::str::contains("subdir"));
}

#[test]
fn discover_dot_ito_subdir() {
    let fx = Fixture::new();
    in_fixture(&fx)
        .arg("dotsubdir")
        .assert()
        .success()
        .stdout(predicate::str::contains("dotsubdir"));
}

#[test]
fn discover_not_found_exits_nonzero() {
    let fx = Fixture::new();
    in_fixture(&fx)
        .arg("nonexistent")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no script found for 'nonexistent'",
        ));
}

#[test]
fn discover_ambiguous_exits_nonzero() {
    let fx = Fixture::new();
    in_fixture(&fx)
        .arg("ambiguous")
        .assert()
        .failure()
        .stderr(predicate::str::contains("ambiguous script for 'ambiguous'"));
}

#[test]
fn reserved_name_without_file_exits_nonzero() {
    let fx = Fixture::new();
    in_fixture(&fx).arg("exec").assert().failure();
}

// --- ito::flag / ito::opt / ito::try_opt ---

#[test]
fn script_flag_present() {
    ito()
        .args(["exec", r#"print(ito::flag("dry-run"))"#, "+dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("true"));
}

#[test]
fn script_flag_absent() {
    ito()
        .args(["exec", r#"print(ito::flag("dry-run"))"#])
        .assert()
        .success()
        .stdout(predicate::str::contains("false"));
}

#[test]
fn script_opt_present() {
    ito()
        .args(["exec", r#"print(ito::opt("env"))"#, "env=staging"])
        .assert()
        .success()
        .stdout(predicate::str::contains("staging"));
}

#[test]
fn script_opt_absent_exits_nonzero() {
    ito()
        .args(["exec", r#"print(ito::opt("env"))"#])
        .assert()
        .failure();
}

#[test]
fn script_try_opt_present() {
    ito()
        .args(["exec", r#"print(ito::try_opt("tag"))"#, "tag=v1.2.3"])
        .assert()
        .success()
        .stdout(predicate::str::contains("v1.2.3"));
}

#[test]
fn script_try_opt_absent_returns_unit() {
    // Avoid `=` in the inline script — it would be stripped as a key=value script opt.
    ito()
        .args(["exec", r#"if ito::try_opt("tag") == () { print("absent") }"#])
        .assert()
        .success()
        .stdout(predicate::str::contains("absent"));
}

#[test]
fn script_flags_stripped_before_clap() {
    // +flag and key=value before exec must not confuse clap
    ito()
        .args([
            "exec",
            r#"print(ito::flag("x") + " " + ito::opt("k"))"#,
            "+x",
            "k=hello",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("true hello"));
}

#[test]
fn double_dash_does_not_stop_script_flag_scanning() {
    // +flag after -- is still consumed as a script flag
    ito()
        .args(["exec", r#"print(ito::flag("x"))"#, "--", "+x"])
        .assert()
        .success()
        .stdout(predicate::str::contains("true"));
}

#[test]
fn clap_style_arg_not_treated_as_opt() {
    // --key=val must not be consumed as a script opt
    ito()
        .args(["exec", r#"print(ito::try_opt("key") == ())"#, "--key=val"])
        .assert()
        // clap will reject --key=val as unknown; the point is it isn't a script opt
        .failure();
}
