//! Integration tests for `proc.trap`/`proc.untrap`/`proc.exit`.
//!
//! These send a *real* signal to the running process (`kill(getpid(), sig)`,
//! via a spawned `kill` subprocess so the signal genuinely arrives through
//! the kernel rather than being simulated). `kill` targets a whole process,
//! and Rust normally runs every `#[test]` in one file as a thread within one
//! shared process/pid, so two tests using the same signal number could
//! observe each other's deliveries. Putting each such test in its own
//! `tests/*.rs` file sidesteps that: cargo gives every integration test file
//! its own binary and so its own process. Do not move these into a shared
//! file, and do not add a second test here that traps the same signal number
//! as another test in this file.
//!
//! `host_io` (and so `proc` itself) exists only under `ito`/`irun`, so these
//! all drive the compiled `ito` binary via `assert_cmd`.

use assert_cmd::Command;
use predicates::prelude::*;

fn ito() -> Command {
    Command::cargo_bin("ito").unwrap()
}

/// A script fragment that signals this same process and gives the trap
/// worker a real chance to run before the script (and so the process) ends.
///
/// `task.wait` yields back to the runtime, which is what lets a `spawn_local`
/// trap worker actually be polled -- a script with no `await` between
/// sending the signal and exiting could race its own trap the same way an
/// external `kill` immediately after `proc.trap` would (see the `settle`
/// helper this replaces from the unit tests).
fn signal_self_and_wait(name: &str) -> String {
    format!(
        r#"proc.command({{ "kill", "-{name}", tostring(ito.pid()) }}):run()
           for _ = 1, 200 do
               task.wait("5ms")
           end"#
    )
}

#[test]
fn trap_runs_on_delivery_and_counts_it() {
    let script = format!(
        r#"
        local seen = {{}}
        proc.trap(proc.SIGUSR1, function(sig)
            seen.signal = sig
            seen.count = (seen.count or 0) + 1
        end)
        {}
        print("signal=" .. tostring(seen.signal) .. " count=" .. tostring(seen.count))
        "#,
        signal_self_and_wait("USR1")
    );
    ito()
        .args(["-s", &script])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "signal={} count=1",
            libc::SIGUSR1
        )));
}

#[test]
fn retrapping_replaces_rather_than_stacks() {
    let script = format!(
        r#"
        local hits = {{}}
        proc.trap(proc.SIGUSR2, function() table.insert(hits, "first") end)
        proc.trap(proc.SIGUSR2, function() table.insert(hits, "second") end)
        {}
        print("hits=" .. #hits .. " last=" .. hits[#hits])
        "#,
        signal_self_and_wait("USR2")
    );
    ito()
        .args(["-s", &script])
        .assert()
        .success()
        .stdout(predicate::str::contains("hits=1 last=second"));
}

#[test]
fn untrap_silences_the_signal() {
    let script = format!(
        r#"
        local hits = {{}}
        proc.trap(proc.SIGHUP, function() table.insert(hits, true) end)
        proc.untrap(proc.SIGHUP)
        {}
        print("hits=" .. #hits)
        "#,
        signal_self_and_wait("HUP")
    );
    ito()
        .args(["-s", &script])
        .assert()
        .success()
        .stdout(predicate::str::contains("hits=0"));
}

#[test]
fn untrap_then_retrap_reuses_the_worker() {
    let script = format!(
        r#"
        local hits = {{}}
        proc.trap(proc.SIGALRM, function() table.insert(hits, "first") end)
        proc.untrap(proc.SIGALRM)
        proc.trap(proc.SIGALRM, function() table.insert(hits, "second") end)
        {}
        print("hits=" .. #hits .. " last=" .. hits[#hits])
        "#,
        signal_self_and_wait("ALRM")
    );
    ito()
        .args(["-s", &script])
        .assert()
        .success()
        .stdout(predicate::str::contains("hits=1 last=second"));
}

/// A signal nothing has ever trapped keeps its kernel default -- which, for
/// SIGTERM, is to terminate the process. This is the "no built-in reaction"
/// contract `main_with`'s doc comment describes: `ito` installs no handler of
/// its own, so an unrelated SIGTERM still ends the run exactly as it always
/// did, whether or not the script happens to use `proc.trap` for something
/// else.
#[test]
fn an_untrapped_signal_keeps_its_default_disposition() {
    let script = r#"
        proc.trap(proc.SIGUSR1, function() end) -- unrelated signal, irrelevant here
        proc.command({ "kill", "-TERM", tostring(ito.pid()) }):run()
        for _ = 1, 200 do
            task.wait("5ms")
        end
        print("should not print: the process should have died of SIGTERM")
    "#;
    let assert = ito().args(["-s", script]).assert();
    let output = assert.get_output();
    assert!(
        !output.status.success(),
        "an untrapped SIGTERM should have killed the process, got: {output:?}"
    );
}

#[test]
fn proc_exit_terminates_immediately_with_the_given_code() {
    ito()
        .args(["-s", "proc.exit(7) print(\"unreachable\")"])
        .assert()
        .code(7)
        .stdout(predicate::str::contains("unreachable").not());
}

#[test]
fn proc_exit_defaults_to_zero() {
    ito().args(["-s", "proc.exit()"]).assert().code(0);
}

#[test]
fn kill_and_stop_are_rejected_by_the_cli_script_too() {
    ito()
        .args(["-s", "proc.trap(proc.SIGKILL, function() end)"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("cannot be trapped"));
}
