//! `proc` module: execute OS processes from Rhai scripts.
//!
//! - `proc::task("program")` / `proc::task(["program", "arg1", ...])`
//!   creates a `ProcTask` builder (pre-split args preferred).
//! - `proc::run(task)` runs a single task synchronously.
//! - `proc::runner()` returns a batch builder with concurrency, timeouts,
//!   per-result callbacks, and fail-fast control.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use rhai::{
    Array, Dynamic, Engine, EvalAltResult, FnPtr, ImmutableString, Map, Module, NativeCallContext,
};
use serde_json::Value as JsonValue;
use wait_timeout::ChildExt;

type RhaiResult<T> = Result<T, Box<EvalAltResult>>;

fn to_err(msg: impl std::fmt::Display) -> Box<EvalAltResult> {
    msg.to_string().into()
}

// ---------------------------------------------------------------------------
// Capture mode
// ---------------------------------------------------------------------------

/// How a single output stream (stdout or stderr) is handled. Default `Drop`
/// sends the stream to `/dev/null`; `Text` decodes captured bytes as UTF-8
/// (lossy) into a string; `Blob` keeps the raw bytes as a Rhai blob.
#[derive(Clone, Copy, Default, PartialEq)]
enum Capture {
    #[default]
    Drop,
    Text,
    Blob,
}

impl Capture {
    fn is_captured(self) -> bool {
        self != Capture::Drop
    }
}

// ---------------------------------------------------------------------------
// ProcTask
// ---------------------------------------------------------------------------

/// A process invocation to be executed. Cheap to clone (all value types).
#[derive(Clone)]
pub struct ProcTask {
    program: String,
    args: Vec<String>,
    /// Per-task wall-clock timeout in seconds. Default: 10.
    timeout_secs: u64,
    /// Opaque user data returned unchanged in the result map.
    data: Map,
    /// How to capture stdout. Default: dropped.
    capture_stdout: Capture,
    /// How to capture stderr. Default: dropped.
    capture_stderr: Capture,
}

impl ProcTask {
    fn new(program: String, args: Vec<String>) -> Self {
        Self {
            program,
            args,
            timeout_secs: 10,
            data: Map::new(),
            capture_stdout: Capture::Drop,
            capture_stderr: Capture::Drop,
        }
    }
}

// ---------------------------------------------------------------------------
// ProcRunner internal state
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
enum FailFast {
    #[default]
    Never,
    /// Stop on first non-zero exit code.
    OnNonZero,
    /// Stop when the predicate returns `false`.
    Custom(FnPtr),
}

#[derive(Clone)]
struct RunnerState {
    jobs: Vec<ProcTask>,
    /// Max parallel OS processes. Default: 1 (sequential).
    concurrency: usize,
    on_result: Option<FnPtr>,
    fail_fast: FailFast,
    /// Total batch wall-clock timeout in seconds. Default: 3600.
    batch_timeout_secs: u64,
}

impl Default for RunnerState {
    fn default() -> Self {
        Self {
            jobs: Vec::new(),
            concurrency: 1,
            on_result: None,
            fail_fast: FailFast::Never,
            batch_timeout_secs: 3600,
        }
    }
}

/// Batch runner handle. Cheap to clone: all clones share the same state.
#[derive(Clone)]
pub struct ProcRunner {
    state: Rc<RefCell<RunnerState>>,
}

// ---------------------------------------------------------------------------
// Core execution helper
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Thread-safe result type
// ---------------------------------------------------------------------------

/// Thread-safe result sent from worker threads. Uses `JsonValue` because
/// `rhai::Dynamic` / `rhai::Map` are not `Send` (they contain `Rc`). Raw
/// stream bytes are carried as `Vec<u8>` and decoded into the result map
/// (string or blob) per the requested capture mode.
struct TaskResult {
    ok: bool,
    exit_code: i64,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    capture_stdout: Capture,
    capture_stderr: Capture,
    /// User data serialised to JSON for cross-thread transfer.
    data_json: JsonValue,
    timed_out: bool,
}

impl TaskResult {
    fn into_map(self) -> RhaiResult<Map> {
        let data_dyn: Dynamic = rhai::serde::to_dynamic(self.data_json)?;
        let mut m = Map::new();
        m.insert("ok".into(), Dynamic::from(self.ok));
        m.insert("exit_code".into(), Dynamic::from(self.exit_code));
        m.insert("data".into(), data_dyn);
        m.insert("timed_out".into(), Dynamic::from(self.timed_out));
        insert_stream(&mut m, "stdout", self.stdout, self.capture_stdout);
        insert_stream(&mut m, "stderr", self.stderr, self.capture_stderr);
        Ok(m)
    }
}

/// Insert a captured stream into the result map under `key`. Dropped streams
/// add no key at all; text streams add a (lossy UTF-8) string; blob streams
/// add a `Blob`.
fn insert_stream(m: &mut Map, key: &str, bytes: Vec<u8>, mode: Capture) {
    match mode {
        Capture::Drop => {}
        Capture::Text => {
            m.insert(
                key.into(),
                Dynamic::from(String::from_utf8_lossy(&bytes).into_owned()),
            );
        }
        Capture::Blob => {
            m.insert(key.into(), Dynamic::from_blob(bytes));
        }
    }
}

/// Serialised form of `ProcTask` that is `Send` (no Rhai types).
struct SendTask {
    program: String,
    args: Vec<String>,
    timeout_secs: u64,
    data_json: JsonValue,
    capture_stdout: Capture,
    capture_stderr: Capture,
}

impl SendTask {
    fn from_task(task: &ProcTask) -> RhaiResult<Self> {
        let data_json = rhai::serde::from_dynamic::<JsonValue>(&Dynamic::from(task.data.clone()))?;
        Ok(Self {
            program: task.program.clone(),
            args: task.args.clone(),
            timeout_secs: task.timeout_secs,
            data_json,
            capture_stdout: task.capture_stdout,
            capture_stderr: task.capture_stderr,
        })
    }
}

// ---------------------------------------------------------------------------
// Core execution helper
// ---------------------------------------------------------------------------

/// Run a single task to completion. Never panics; errors are folded into
/// the result (ok=false, stderr=error message).
fn run_send_task(task: SendTask) -> TaskResult {
    use std::process::{Command, Stdio};

    // Errors are reported as stderr text regardless of the requested capture
    // mode, so the message is always visible to the caller.
    let err = |msg: String| TaskResult {
        ok: false,
        exit_code: -1,
        stdout: Vec::new(),
        stderr: msg.into_bytes(),
        capture_stdout: Capture::Drop,
        capture_stderr: Capture::Text,
        data_json: task.data_json.clone(),
        timed_out: false,
    };

    let stdio = |mode: Capture| {
        if mode.is_captured() {
            Stdio::piped()
        } else {
            Stdio::null()
        }
    };

    let mut child = match Command::new(&task.program)
        .args(&task.args)
        .stdin(Stdio::null())
        .stdout(stdio(task.capture_stdout))
        .stderr(stdio(task.capture_stderr))
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return err(format!("proc: spawn '{}': {e}", task.program)),
    };

    let timeout = Duration::from_secs(task.timeout_secs);
    let status_opt = match child.wait_timeout(timeout) {
        Ok(s) => s,
        Err(e) => return err(format!("proc: wait_timeout: {e}")),
    };

    let timed_out = status_opt.is_none();
    if timed_out {
        let _ = child.kill();
    }

    // Drain pipes and reap. After kill() the child's pipes are closed,
    // so wait_with_output() returns promptly.
    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return err(format!("proc: wait_with_output: {e}")),
    };

    let exit_code: i64 = if timed_out {
        -1
    } else {
        output.status.code().unwrap_or(-1) as i64
    };

    TaskResult {
        ok: exit_code == 0 && !timed_out,
        exit_code,
        stdout: output.stdout,
        stderr: output.stderr,
        capture_stdout: task.capture_stdout,
        capture_stderr: task.capture_stderr,
        data_json: task.data_json,
        timed_out,
    }
}

fn run_task(task: &ProcTask) -> RhaiResult<Map> {
    let send = SendTask::from_task(task)?;
    run_send_task(send).into_map()
}

// ---------------------------------------------------------------------------
// Batch runner execution
// ---------------------------------------------------------------------------

fn runner_run(ctx: &NativeCallContext, runner: &mut ProcRunner) -> RhaiResult<Array> {
    let state = runner.state.borrow().clone();
    let jobs = state.jobs;
    let concurrency = state.concurrency.max(1);
    let on_result = state.on_result;
    let fail_fast = state.fail_fast;
    let batch_timeout = Duration::from_secs(state.batch_timeout_secs);

    // Serialise tasks to Send-safe types up front (errors surface here,
    // before any threads are spawned).
    let send_tasks: Vec<SendTask> = jobs
        .iter()
        .map(SendTask::from_task)
        .collect::<RhaiResult<Vec<_>>>()?;

    let deadline = Instant::now() + batch_timeout;
    let (tx, rx) = mpsc::channel::<TaskResult>();

    let mut results: Array = Vec::new();
    let mut next_job: usize = 0;
    let mut in_flight: usize = 0;
    let mut halted = false;

    // We iterate send_tasks by index. Use a Vec wrapped in an Arc to share
    // ownership with threads without moving. Actually simpler: just pass the
    // task by move into each thread individually via index.
    let send_tasks = std::sync::Arc::new(send_tasks);

    loop {
        // Dispatch jobs up to concurrency limit.
        while !halted && in_flight < concurrency && next_job < send_tasks.len() {
            let task = {
                // Clone the individual task out of the Arc'd vec.
                let t = &send_tasks[next_job];
                SendTask {
                    program: t.program.clone(),
                    args: t.args.clone(),
                    timeout_secs: t.timeout_secs,
                    data_json: t.data_json.clone(),
                    capture_stdout: t.capture_stdout,
                    capture_stderr: t.capture_stderr,
                }
            };
            next_job += 1;
            let tx2 = tx.clone();
            thread::spawn(move || {
                let _ = tx2.send(run_send_task(task));
            });
            in_flight += 1;
        }

        if in_flight == 0 {
            break;
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        let task_result = match rx.recv_timeout(remaining) {
            Ok(r) => r,
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        in_flight -= 1;

        let result_map = task_result.into_map()?;

        // Invoke on_result callback on the main (Rhai) thread.
        if let Some(ref cb) = on_result {
            let _ = cb.call_within_context::<Dynamic>(ctx, (Dynamic::from(result_map.clone()),))?;
        }

        // Evaluate fail_fast predicate.
        let should_halt = match &fail_fast {
            FailFast::Never => false,
            FailFast::OnNonZero => !result_map
                .get("ok")
                .and_then(|d| d.as_bool().ok())
                .unwrap_or(false),
            FailFast::Custom(pred) => {
                // Predicate returns true = continue, false = halt.
                !pred.call_within_context::<bool>(ctx, (Dynamic::from(result_map.clone()),))?
            }
        };

        results.push(Dynamic::from(result_map));

        if should_halt {
            // Stop dispatching new jobs. Continue draining already
            // in-flight threads to avoid leaking them.
            halted = true;
        }
    }

    // Threads will finish on their own per-task timeouts.

    Ok(results)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn register(engine: &mut Engine) {
    engine.register_type_with_name::<ProcTask>("ProcTask");
    engine.register_type_with_name::<ProcRunner>("ProcRunner");

    // --- ProcTask builder methods (chainable: mutate and return the same
    // handle, so they take the `with_*` prefix) ---

    // .with_timeout(secs) -> ProcTask
    engine.register_fn("with_timeout", |task: &mut ProcTask, secs: rhai::INT| {
        task.timeout_secs = secs.max(0) as u64;
        task.clone()
    });

    // .with_data(map) -> ProcTask
    engine.register_fn("with_data", |task: &mut ProcTask, map: Map| {
        task.data = map;
        task.clone()
    });

    // Capture builders. By default both streams are dropped (sent to
    // /dev/null) and absent from the result map. These opt in per stream,
    // choosing text (UTF-8 lossy string) or blob (raw bytes) decoding.

    // .with_capture() -> ProcTask  (both streams as text)
    engine.register_fn("with_capture", |task: &mut ProcTask| {
        task.capture_stdout = Capture::Text;
        task.capture_stderr = Capture::Text;
        task.clone()
    });

    // .with_capture_stdout() -> ProcTask  (stdout as text)
    engine.register_fn("with_capture_stdout", |task: &mut ProcTask| {
        task.capture_stdout = Capture::Text;
        task.clone()
    });

    // .with_capture_stderr() -> ProcTask  (stderr as text)
    engine.register_fn("with_capture_stderr", |task: &mut ProcTask| {
        task.capture_stderr = Capture::Text;
        task.clone()
    });

    // .with_capture_blob() -> ProcTask  (both streams as raw bytes)
    engine.register_fn("with_capture_blob", |task: &mut ProcTask| {
        task.capture_stdout = Capture::Blob;
        task.capture_stderr = Capture::Blob;
        task.clone()
    });

    // .with_capture_stdout_blob() -> ProcTask  (stdout as raw bytes)
    engine.register_fn("with_capture_stdout_blob", |task: &mut ProcTask| {
        task.capture_stdout = Capture::Blob;
        task.clone()
    });

    // .with_capture_stderr_blob() -> ProcTask  (stderr as raw bytes)
    engine.register_fn("with_capture_stderr_blob", |task: &mut ProcTask| {
        task.capture_stderr = Capture::Blob;
        task.clone()
    });

    // --- ProcRunner builder methods (chainable: mutate and return the same
    // handle, so they take the `with_*` prefix) ---

    // .with_job(task) -> ProcRunner
    engine.register_fn("with_job", |runner: &mut ProcRunner, task: ProcTask| {
        runner.state.borrow_mut().jobs.push(task);
        runner.clone()
    });

    // .with_job([tasks]) -> ProcRunner
    engine.register_fn(
        "with_job",
        |runner: &mut ProcRunner, tasks: Array| -> RhaiResult<ProcRunner> {
            let mut state = runner.state.borrow_mut();
            for d in tasks {
                let t = d
                    .try_cast::<ProcTask>()
                    .ok_or_else(|| -> Box<EvalAltResult> {
                        "proc: runner.job: array elements must be ProcTask values".into()
                    })?;
                state.jobs.push(t);
            }
            drop(state);
            Ok(runner.clone())
        },
    );

    // .with_concurrency(n) -> ProcRunner
    engine.register_fn(
        "with_concurrency",
        |runner: &mut ProcRunner, n: rhai::INT| {
            runner.state.borrow_mut().concurrency = n.max(1) as usize;
            runner.clone()
        },
    );

    // .with_on_result(|result| {}) -> ProcRunner
    engine.register_fn("with_on_result", |runner: &mut ProcRunner, cb: FnPtr| {
        runner.state.borrow_mut().on_result = Some(cb);
        runner.clone()
    });

    // .with_fail_fast() -> ProcRunner  (halt on first non-zero exit)
    engine.register_fn("with_fail_fast", |runner: &mut ProcRunner| {
        runner.state.borrow_mut().fail_fast = FailFast::OnNonZero;
        runner.clone()
    });

    // .with_fail_fast(|result| bool) -> ProcRunner  (halt when lambda returns false)
    engine.register_fn("with_fail_fast", |runner: &mut ProcRunner, pred: FnPtr| {
        runner.state.borrow_mut().fail_fast = FailFast::Custom(pred);
        runner.clone()
    });

    // .with_timeout(secs) -> ProcRunner  (batch-level timeout)
    engine.register_fn(
        "with_timeout",
        |runner: &mut ProcRunner, secs: rhai::INT| {
            runner.state.borrow_mut().batch_timeout_secs = secs.max(0) as u64;
            runner.clone()
        },
    );

    // .run() -> Array  (NativeCallContext for FnPtr callbacks)
    engine.register_fn(
        "run",
        |ctx: NativeCallContext, runner: &mut ProcRunner| -> RhaiResult<Array> {
            runner_run(&ctx, runner)
        },
    );

    // --- Static module ---

    let mut module = Module::new();

    // proc::task("program") -> ProcTask
    module.set_native_fn("task", |program: ImmutableString| -> RhaiResult<ProcTask> {
        if program.is_empty() {
            return Err(to_err("proc::task: program must not be empty"));
        }
        Ok(ProcTask::new(program.to_string(), vec![]))
    });

    // proc::task(["program", "arg1", ...]) -> ProcTask
    module.set_native_fn("task", |argv: Array| -> RhaiResult<ProcTask> {
        if argv.is_empty() {
            return Err(to_err("proc::task: argv must not be empty"));
        }
        let strs: Vec<String> = argv
            .into_iter()
            .map(|d| {
                d.into_string().map_err(|_| -> Box<EvalAltResult> {
                    "proc::task: argv elements must be strings".into()
                })
            })
            .collect::<RhaiResult<Vec<String>>>()?;
        Ok(ProcTask::new(strs[0].clone(), strs[1..].to_vec()))
    });

    // proc::run(task) -> Map
    module.set_native_fn("run", |task: ProcTask| -> RhaiResult<Map> {
        run_task(&task)
    });

    // proc::runner() -> ProcRunner
    module.set_native_fn("runner", || -> RhaiResult<ProcRunner> {
        Ok(ProcRunner {
            state: Rc::new(RefCell::new(RunnerState::default())),
        })
    });

    engine.register_static_module("proc", module.into());
}
