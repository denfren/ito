//! A reader that goes away (`| head`) ends the run with 141 (128 + SIGPIPE),
//! quietly, and a `pcall` around the write cannot hide it.

use std::process::{Command, Stdio};

fn run_into_dead_pipe(src: &str) -> (Option<i32>, String) {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("s.luau");
    std::fs::write(&script, src).unwrap();
    let mut child = Command::new(assert_cmd::cargo::cargo_bin("ito"))
        .arg(&script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Drop the read end without reading: the first write that lands raises EPIPE.
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stderr).into())
}

#[test]
fn broken_pipe_exits_141_quietly() {
    let (code, stderr) = run_into_dead_pipe("for i = 1, 100000 do print(i) end");
    assert_eq!(code, Some(141));
    assert!(stderr.is_empty(), "stderr: {stderr}");
}

#[test]
fn pcall_cannot_hide_a_broken_pipe() {
    let (code, _) = run_into_dead_pipe("for i = 1, 100000 do pcall(print, i) end");
    assert_eq!(code, Some(141));
}
