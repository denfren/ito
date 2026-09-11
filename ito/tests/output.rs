//! Output surfaces: vanilla `print` in both binaries, the `io` handles and
//! stdin in `ito`, and what `ied -q` covers.

use assert_cmd::Command;
use assert_cmd::assert::Assert;
use predicates::prelude::*;

fn ito() -> Command {
    Command::cargo_bin("ito").unwrap()
}

fn ied() -> Command {
    Command::cargo_bin("ied").unwrap()
}

trait RunScript {
    fn run_script(&mut self, src: &str) -> Assert;
}

impl RunScript for Command {
    fn run_script(&mut self, src: &str) -> Assert {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("s.luau");
        std::fs::write(&script, src).unwrap();
        self.arg(&script).assert()
    }
}

#[test]
fn print_is_vanilla_in_both_binaries() {
    const SRC: &str = r#"print("a", 1, true, nil)
print(setmetatable({}, {__tostring = function() return "custom" end}))
print({})"#;
    for mut cmd in [ito(), ied()] {
        cmd.run_script(SRC)
            .success()
            .stdout(predicate::str::starts_with("a\t1\ttrue\tnil\ncustom\ntable: 0x"));
    }
}

#[test]
fn ied_quiet_silences_print() {
    ied()
        .arg("-q")
        .run_script(r#"print("hi")"#)
        .success()
        .stdout("");
}

#[test]
fn ied_report_flags_imply_quiet() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "x").unwrap();
    for flag in ["-l", "-d"] {
        let out = ied()
            .args([flag, "-C"])
            .arg(dir.path())
            .run_script(r#"print("noise") fs.write("f.txt", "y")"#)
            .success();
        let out = String::from_utf8(out.get_output().stdout.clone()).unwrap();
        assert!(!out.contains("noise"), "{flag}: {out}");
        assert!(out.contains("f.txt"), "{flag}: {out}");
    }
}

#[test]
fn ied_has_no_io() {
    ied()
        .run_script("print(typeof(io))")
        .success()
        .stdout("nil\n");
}

#[test]
fn ito_has_no_quiet_flag() {
    ito().args(["-q", "x.luau"]).assert().failure().code(2);
}

#[test]
fn ito_io_handles_write_to_their_streams() {
    ito()
        .run_script(r#"io.stdout:write("o", 1):write("\n") io.stderr:write("e\n") io.write("w\n")"#)
        .success()
        .stdout("o1\nw\n")
        .stderr("e\n");
}

#[test]
fn ito_io_write_rejects_non_strings() {
    ito()
        .run_script("io.write({})")
        .code(3)
        .stderr(predicate::str::contains("string expected, got table"));
}

#[test]
fn ito_reads_stdin() {
    ito()
        .write_stdin("one\ntwo\nthree\n")
        .run_script(
            r#"print(io.read())
               print(io.stdin:read("L"))
               for l in io.lines() do print("[" .. l .. "]") end
               print(io.read(), io.read("a"))"#,
        )
        .success()
        .stdout("one\ntwo\n\n[three]\nnil\t\n");
}

#[test]
fn ito_read_count() {
    ito()
        .write_stdin("abcdef")
        .run_script(r#"print(io.read(2), io.read(10), io.read(1))"#)
        .success()
        .stdout("ab\tcdef\tnil\n");
}
