# `proc` — process execution

The `proc` module lets a script execute arbitrary processes, so it is
**not registered by default**. Pass `--unsafe-proc` to `ito exec` / `ito
run` to enable it; without the flag, `import`ing or calling `proc::*`
fails with a "Module not found" error.

| Function | Effect |
| --- | --- |
| `proc::task("program")` | Create a task for a single binary with no arguments. |
| `proc::task(["program", "arg1", ...])` | Create a task with pre-split arguments (preferred — no shell). |
| `proc::run(task)` | Run a single task synchronously. Returns a result map. |
| `proc::runner()` | Start a batch runner builder. |

## ProcTask builder

These are chainable `with_*` methods: each mutates the task and returns
it (see [Method naming conventions](../conventions.md)).

| Method | Effect |
| --- | --- |
| `.with_timeout(seconds)` | Per-task wall-clock timeout in seconds (default: 10). |
| `.with_data(map)` | Attach a custom map returned unchanged in the result. |
| `.with_capture()` | Capture **both** streams as text (UTF-8 lossy strings). |
| `.with_capture_stdout()` | Capture stdout as text. |
| `.with_capture_stderr()` | Capture stderr as text. |
| `.with_capture_blob()` | Capture **both** streams as raw bytes (blobs). |
| `.with_capture_stdout_blob()` | Capture stdout as raw bytes. |
| `.with_capture_stderr_blob()` | Capture stderr as raw bytes. |

## Output capture

By default **both stdout and stderr are dropped** (routed to `/dev/null`)
and are absent from the result map. Opt in per stream with the
`with_capture*` builders; calling a non-`_blob` and a `_blob` builder for
the same stream is last-wins. Capturing keeps a stream's bytes in memory until the task
completes (see the pipe-buffer limitation below).

## Result map

Every completed task produces a map with these keys:

| Key | Type | Description |
| --- | --- | --- |
| `ok` | bool | `true` when `exit_code == 0` and the task did not time out. |
| `exit_code` | int | Raw OS exit code; `-1` for timeout or signal. |
| `data` | map | The map attached via `.with_data()`, or `#{}` if none. |
| `timed_out` | bool | `true` when the per-task timeout fired. |
| `stdout` | string / blob | Present only when stdout is captured; string for text, blob for `_blob`. |
| `stderr` | string / blob | Present only when stderr is captured; string for text, blob for `_blob`. |

A dropped stream contributes **no key** — use `"stdout" in result` to
test for presence. A spawn/wait error sets `ok=false`, `exit_code=-1` and
always reports the message as a text `stderr`.

## ProcRunner builder

The `with_*` methods are chainable (each mutates the runner and returns
it); `.run()` is the terminal action (see
[Method naming conventions](../conventions.md)).

| Method | Effect |
| --- | --- |
| `.with_job(task)` | Add a single task. |
| `.with_job([tasks])` | Add an array of tasks. |
| `.with_concurrency(n)` | Max parallel processes (default: 1 — sequential). |
| `.with_timeout(seconds)` | Total batch wall-clock timeout (default: 3600). |
| `.with_on_result(\|result\| { })` | Called with each result map as it completes. |
| `.with_fail_fast()` | Stop dispatching after the first non-zero exit. |
| `.with_fail_fast(\|result\| bool)` | Stop dispatching when the predicate returns `false`. |
| `.run()` | Execute all jobs; returns an array of result maps. |

The runner dispatches up to `concurrency` OS threads at a time. Results
arrive in completion order (not submission order). When `fail_fast` fires,
already-started processes run to their own per-task timeout; only pending
unstarted jobs are cancelled.

Child processes get an empty stdin (`/dev/null`): any read returns EOF
immediately. The runner is non-interactive and never passes through `ito`'s
own stdin.

**Known limitation:** captured streams are collected through OS pipes. A
captured process that writes more than ~64 KB without having its output
drained may fill the pipe buffer and block until `wait_timeout` fires.
Dropped streams (the default) go to `/dev/null` and never block.

## Examples

```rhai
// Fire-and-forget — no capture, only the exit status matters
let r = proc::run(proc::task(["git", "fetch", "--quiet"]));
if !r.ok { print(`git fetch failed (exit ${r.exit_code})`); }

// Single task — capture stdout/stderr as text
let r = proc::run(proc::task(["git", "status", "--short"]).with_capture());
if r.ok {
    print(r.stdout);
} else {
    print(`git failed (exit ${r.exit_code}): ${r.stderr}`);
}

// Capture binary output as a blob
let r = proc::run(proc::task(["cat", "logo.png"]).with_capture_stdout_blob());
if r.ok { fs::write("/copy.png", r.stdout); }

// Task with custom data and timeout
let r = proc::run(
    proc::task(["curl", "-sf", "https://example.com"])
        .with_timeout(5)
        .with_capture_stdout()
        .with_data(#{ url: "https://example.com" })
);
if r.timed_out { print(`timed out: ${r.data.url}`); }

// Batch: run several scripts in parallel
let tasks = ["a.sh", "b.sh", "c.sh"].map(|s| {
    proc::task(["/bin/sh", s]).with_capture_stderr().with_data(#{ script: s })
});

let results = proc::runner()
    .with_job(tasks)
    .with_concurrency(3)
    .with_on_result(|r| {
        if !r.ok { print(`FAILED ${r.data.script}: ${r.stderr}`); }
    })
    .with_fail_fast()
    .run();

print(`${results.len()} tasks completed`);
```
