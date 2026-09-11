//! Cooperative tasks, so a script can overlap I/O instead of serializing it.
//!
//! The I/O modules (`proc`, `ssh`, `aws.ec2`) are async underneath, but a
//! plain call still runs to completion before the next statement. `task.spawn`
//! puts a call on its own Lua thread so several can be in flight at once:
//!
//! ```lua
//! local handles = {}
//! for _, h in ipairs({ "a", "b", "c" }) do
//!     table.insert(handles, task.spawn(function()
//!         return ssh.connect(h):command({ "uptime" }):stdout(proc.string()):run()
//!     end))
//! end
//! local a, b, c = task.await(handles)
//! ```
//!
//! ```lua
//! task.spawn(f, ...)       -- start f(...); returns a handle
//! task.await(h, ...)       -- wait for handles, return their results
//! task.poll(h)             -- -> ready, results... ; never blocks
//! task.status(h)           -- "pending" | "done" | "failed" | "cancelled"
//! task.cancel(h)           -- abort a pending task
//! task.wait(duration)      -- non-blocking sleep
//! task.timeout(d, f, ...)  -- run f(...) under a deadline
//! task.TIMEOUT             -- the sentinel task.timeout returns on expiry
//! task.pool(n)             -- bounded concurrency, results in completion order
//! ```
//!
//! # Pools
//!
//! `task.spawn` starts work immediately, so a loop over a thousand hosts opens
//! a thousand connections. A pool bounds that, and collects results in the
//! order they *finish*:
//!
//! ```lua
//! local pool = task.pool(8)
//! for _, host in ipairs(hosts) do
//!     pool:spawn(function() return host, probe(host) end)
//! end
//! while not pool:done() do
//!     local host, result = pool:await()
//!     print(host, result)
//! end
//! ```
//!
//! A pool is deliberately *not* a gated `task.spawn`: `pool:spawn` returns
//! nothing, because the pool owns the result and `await`/`poll` are the only
//! way to it. That leaves one collection path rather than two that would have
//! to agree about who claimed a failure. The cost is that a result carries no
//! handle to identify it, so a task returns its own identity -- which is
//! better than an opaque `task#7` anyway, the script choosing what names the
//! work.
//!
//! Results arrive in **completion order**, which is the whole point over
//! `task.await(list)`: a slow host does not hold up the ones behind it.
//! Nothing lines up with the spawn loop.
//!
//! `await`/`poll` **raise** on a drained pool rather than returning `nil`,
//! since `nil` would be indistinguishable from a task that returned `nil`
//! first -- the same ambiguity `task.TIMEOUT` exists to avoid. `done()` and
//! `count()` are one fact read two ways (`done()` is `count() == 0`); guard the
//! loop with either.
//!
//! Pooled tasks are ordinary tasks: the drain covers them, so one that fails
//! without ever being collected fails the run exactly as an un-awaited
//! `task.spawn` does. `pool:cancel()` is therefore the only way to abandon a
//! sweep *without* failing the run, cancellation being exempt from drain
//! reporting.
//!
//! A pool is **not reentrant**. A pooled task holds its permit while parked, so
//! awaiting a pool from inside pooled work would wedge the pool once
//! `concurrency` tasks did it -- with no error and no deadline to break it. So
//! `await`/`poll` raise when called from within *any* pool's task, rather than
//! hanging.
//!
//! A duration is read by `time.parse_delta`: a number of seconds or a string
//! (`"30s"`, `"5min"`, `"1h 30min"`), so a deadline reads as what it is.
//!
//! `task.timeout` returns what `f` returned, unchanged, or the `task.TIMEOUT`
//! sentinel if the deadline passed first. An error raised by `f` still
//! propagates, the way `await` reports one -- only the deadline produces the
//! sentinel, which is what makes a `== task.TIMEOUT` test mean "too slow" and
//! nothing else. On expiry the task is cancelled, with the reach that has: a
//! local `proc` child is abandoned rather than killed, and a remote `ssh`
//! command keeps running on its host, since SSH has no signal channel.
//!
//! Concurrency here is *not* parallelism: mlua's Lua state is single-threaded,
//! so tasks are pinned to the runtime's one thread (`tokio::task::spawn_local`)
//! and interleave only where they await. That is exactly what I/O-bound work
//! needs, and it keeps every value a script touches free of a `Send` bound.
//!
//! A task starts making progress as soon as the runtime next gets control --
//! which is at the spawner's own next await point, not at the `spawn` call. So
//! `spawn` never blocks, and an error raised by the body surfaces at
//! `await`/`poll` (or, for a task nobody awaits, at drain time), the way
//! `coroutine.resume` reports failure rather than raising at the spawn site.
//!
//! An `await` over several handles settles every one of them before raising,
//! and claims all their errors, not just the one it reports. Returning early on
//! the first failure would leave the rest of the batch unawaited, and an
//! unawaited failure fails the run from the drain -- outside whatever `pcall`
//! the script guarded the `await` with.
//!
//! Tasks are drained before the host flushes the VFS, so a task spawned purely
//! for its side effects still lands its `fs.write`s even if nobody awaits it. A
//! task that fails without ever being awaited is logged and fails the run; a
//! cancelled task is neither driven to completion nor reported.
//!
//! `require` from inside a task is safe despite the requirer being a single
//! mutable navigator: Luau resolves an import synchronously, within one resume,
//! so no other task can be running between the requirer's `reset` and the
//! `loader` call that ends the sequence.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use mlua::{
    AnyUserData, Lua, MultiValue, Result as LuaResult, Table, UserData, UserDataMethods, Value,
    Variadic,
};

/// Where a spawned task is in its life.
enum State {
    /// Still running, driven by its own `spawn_local` task.
    Pending,
    /// Finished successfully; holds the values it returned.
    Done(MultiValue),
    /// Raised an error. `reported` records whether anyone has seen it, so an
    /// un-awaited failure can be surfaced at drain time without repeating one
    /// already delivered to an `await`/`poll`.
    Failed { error: mlua::Error, reported: bool },
    /// Cancelled before it finished: not driven, not reported.
    Cancelled,
}

/// The scheduler's view of one task, shared with its Lua handle.
struct Task {
    /// Stable identity, for the handle's `tostring` and for log messages.
    id: u64,
    /// Belongs to a pool, so log lines can say more than an opaque `task#N`.
    pooled: bool,
    state: State,
    /// Handle on the `spawn_local` task driving this one, used by `cancel`.
    ///
    /// `None` once the task has settled -- aborting a finished task is a no-op
    /// and holding the handle would only keep it alive.
    driver: Option<tokio::task::JoinHandle<()>>,
    /// Wakers of `await`s parked on this task, woken when it settles.
    ///
    /// Several tasks may await the same handle, so this is a list rather than a
    /// single slot.
    waiters: Vec<std::task::Waker>,
}

impl Task {
    /// Move the task to its final state and release anyone waiting on it.
    fn settle(&mut self, state: State) {
        self.state = state;
        self.driver = None;
        for waker in self.waiters.drain(..) {
            waker.wake();
        }
    }
}

/// Registry of every task the script has spawned.
///
/// Shared between the module's closures and the host's [`Tasks`] drain handle.
/// Keyed by id, so iteration follows spawn order -- the order un-awaited
/// failures get reported in.
#[derive(Default)]
struct Registry {
    next_id: u64,
    tasks: BTreeMap<u64, Rc<RefCell<Task>>>,
}

/// A Lua-side handle to a spawned task.
///
/// The registry holds the other half of the `Rc`, so dropping every handle does
/// not abandon a task that still has side effects to perform.
#[derive(Clone)]
struct Handle(Rc<RefCell<Task>>);

impl UserData for Handle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(mlua::MetaMethod::ToString, |_, this, ()| {
            let task = this.0.borrow();
            let state = match &task.state {
                State::Pending => "pending",
                State::Done(..) => "done",
                State::Failed { .. } => "failed",
                State::Cancelled => "cancelled",
            };
            Ok(format!("task#{} ({state})", task.id))
        });
    }
}

/// A future that completes when `task` settles.
///
/// Parking on a waker rather than polling the Lua thread directly is what keeps
/// tasks independent: the thread is owned by its `spawn_local` driver, so any
/// number of `await`s can wait on one task without contending for it.
struct Settled(Rc<RefCell<Task>>);

impl std::future::Future for Settled {
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        let mut task = self.0.borrow_mut();
        if matches!(task.state, State::Pending) {
            // Replace this future's previous waker rather than accumulating
            // one per poll: a future may be polled many times before settling.
            let waker = cx.waker();
            if !task.waiters.iter().any(|w| w.will_wake(waker)) {
                task.waiters.push(waker.clone());
            }
            return std::task::Poll::Pending;
        }
        std::task::Poll::Ready(())
    }
}

/// The host's end of the task registry: finishes tasks nobody awaited before
/// the VFS is flushed.
pub struct Tasks {
    registry: Rc<RefCell<Registry>>,
}

impl Tasks {
    /// Wait for every still-pending task to finish.
    ///
    /// Called after the entry chunk (and `main`) returns but *before* the VFS
    /// flush, because a task's buffered writes are only real once it has
    /// finished. Returns the error of the first task that failed without ever
    /// being awaited, having logged each of them.
    pub async fn drain(&self) -> LuaResult<()> {
        let mut first_error = None;
        // A finishing task may spawn more, so keep popping until the registry
        // is empty rather than awaiting a single snapshot. Popping (not
        // scanning) also releases each task, and covers one that failed and
        // settled before the drain ever looked.
        loop {
            let next = self.registry.borrow_mut().tasks.pop_first();
            let Some((_, task)) = next else { break };
            Settled(Rc::clone(&task)).await;
            let mut task = task.borrow_mut();
            let id = task.id;
            let what = if task.pooled { "pooled task" } else { "task" };
            if let State::Failed { error, reported } = &mut task.state {
                if !*reported {
                    *reported = true;
                    tracing::error!("{what}#{id} failed: {error}");
                    first_error.get_or_insert_with(|| error.clone());
                }
            }
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// Read a settled task's result, marking a failure as reported so the drain
/// pass does not repeat it.
///
/// Non-destructive with respect to the value: the result stays on the task, so
/// a `poll` followed by an `await` still yields it.
fn take_settled(task: &Rc<RefCell<Task>>) -> LuaResult<MultiValue> {
    let mut task = task.borrow_mut();
    if let State::Failed { reported, .. } = &mut task.state {
        *reported = true;
    }
    match &task.state {
        State::Done(values) => Ok(values.clone()),
        State::Failed { error, .. } => Err(error.clone()),
        State::Cancelled => Err(mlua::Error::runtime("task was cancelled")),
        State::Pending => unreachable!("take_settled on a pending task"),
    }
}

/// Pull a task out of a Lua value, with an error naming the caller.
fn handle_from(value: &Value, what: &str) -> LuaResult<Rc<RefCell<Task>>> {
    let ud: &AnyUserData = value.as_userdata().ok_or_else(|| {
        mlua::Error::runtime(format!(
            "{what} expects a task handle from task.spawn(), got {}",
            value.type_name()
        ))
    })?;
    let handle = ud.borrow::<Handle>().map_err(|_| {
        mlua::Error::runtime(format!(
            "{what} expects a task handle from task.spawn(), got another userdata"
        ))
    })?;
    Ok(handle.0.clone())
}

/// Collect the handles named by an `await` argument list.
///
/// A single table argument (`task.await({a, b})`) is read as the list of
/// handles, which is what a loop accumulating handles into a table wants.
/// Otherwise every argument is itself a handle.
fn handles_from(args: &Variadic<Value>, what: &str) -> LuaResult<Vec<Rc<RefCell<Task>>>> {
    let handles: Vec<_> = if let [Value::Table(list)] = &args[..] {
        // Walk by length, not `sequence_values`, which stops quietly at the
        // first nil and would drop every handle behind it.
        (1..=list.raw_len())
            .map(|i| handle_from(&list.raw_get::<Value>(i)?, what))
            .collect::<LuaResult<_>>()?
    } else {
        args.iter()
            .map(|v| handle_from(v, what))
            .collect::<LuaResult<_>>()?
    };
    if handles.is_empty() {
        return Err(mlua::Error::runtime(format!(
            "{what} expects at least one task handle"
        )));
    }
    Ok(handles)
}

/// The value `task.timeout` returns when the deadline passed first.
///
/// A userdata rather than a string or a table so identity is the only way to
/// produce it: a script cannot compute something `==` to it by accident, which
/// is what lets `result == task.TIMEOUT` be a reliable test even when the timed
/// function's own return values are arbitrary.
struct Timeout;

impl UserData for Timeout {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(mlua::MetaMethod::ToString, |_, _, ()| {
            Ok("task.TIMEOUT".to_string())
        });
    }
}

/// Read a duration through `time.parse_delta`, the one delta grammar.
///
/// A number is seconds, a string is `"20min"`-style. Negative and out-of-range
/// values are rejected here rather than panicking in `Duration`.
fn duration_from(value: &Value, what: &str) -> LuaResult<Duration> {
    let seconds = super::time::parse_delta_value(value, what)?;
    if seconds < 0.0 {
        return Err(mlua::Error::runtime(format!(
            "{what} expects a non-negative duration, got {seconds}s"
        )));
    }
    Duration::try_from_secs_f64(seconds)
        .map_err(|_| mlua::Error::runtime(format!("{what}: {seconds}s is too long a duration")))
}

/// Start `f(args)` on its own driven thread and register it.
///
/// Shared by `task.spawn` and `task.timeout`, so a timed function is an
/// ordinary task in every respect -- it is in the registry, the drain knows
/// about it, and cancelling it means what cancelling any task means.
fn spawn_task(
    lua: &Lua,
    registry: &Rc<RefCell<Registry>>,
    f: mlua::Function,
    args: MultiValue,
    pool: Option<Rc<RefCell<Pool>>>,
) -> LuaResult<Rc<RefCell<Task>>> {
    // `into_async` is what makes the thread drivable by the host runtime at
    // all, and so is the whole reason this is not just `coroutine.create`: a
    // plain Lua thread that yields on a pending future has nothing to resume
    // it.
    let thread = lua.create_thread(f)?.into_async::<MultiValue>(args)?;

    let id = {
        let mut reg = registry.borrow_mut();
        reg.next_id += 1;
        reg.next_id
    };
    let pooled = pool.is_some();
    let task = Rc::new(RefCell::new(Task {
        id,
        pooled,
        state: State::Pending,
        driver: None,
        waiters: Vec::new(),
    }));
    // Pooled work, or work spawned from inside it, counts as "inside a pool"
    // for the reentrancy check: awaiting a pool from a task that a pooled task
    // spawned and is waiting on deadlocks just the same.
    let tainted = pooled || IN_POOL.get() > 0;
    let reg = Rc::clone(registry);

    // `spawn_local`, so the future need not be `Send` -- an `AsyncThread` never
    // is. It is `'static` though, which is what lets the runtime own it
    // independently of this call.
    let driven = Rc::clone(&task);
    let gate = pool.clone();
    let driver = tokio::task::spawn_local(async move {
        // A pooled task waits for room *before* its body is resumed, so a
        // queued task has run no script code and `spawn` never blocks. The
        // permit is held for exactly the body's lifetime.
        // The `Rc` is cloned out and kept alive for the body's whole lifetime,
        // so the permit (which borrows it) outlives the acquisition. `Arc` and
        // `acquire_owned` would do this too, but nothing here is `Send`.
        let permits = gate.as_ref().map(|p| Rc::clone(&p.borrow().permits));
        let permit = match &permits {
            Some(permits) => match permits.acquire().await {
                Ok(permit) => Some(permit),
                // The semaphore is only closed when the pool is dropped
                // wholesale, at which point there is nothing to run for.
                Err(_) => return,
            },
            None => None,
        };

        // Mark the body as pooled work for the reentrancy check, but only while
        // it is actually *running*. Holding the count across the whole body
        // would raise it while the task sits parked on a `task.wait`, and the
        // main script's own `pool:await` -- on the same thread -- would then
        // look like reentrant pooled work.
        let outcome = if tainted {
            InPool(thread).await
        } else {
            thread.await
        };
        drop(permit);

        let mut task = driven.borrow_mut();
        // A cancel that landed while the thread was mid-flight wins: its
        // result is not the script's to see.
        if !matches!(task.state, State::Pending) {
            return;
        }
        let done = outcome.is_ok();
        task.settle(match outcome {
            Ok(values) => State::Done(values),
            Err(error) => State::Failed {
                error,
                reported: false,
            },
        });
        drop(task);
        // Only a failure the drain may still owe a report stays registered; the
        // handle keeps the result of everything else alive on its own.
        if done {
            reg.borrow_mut().tasks.remove(&id);
        }

        // Queue the result for collection. After `settle`, so a `poll` woken by
        // this cannot see the task still pending.
        if let Some(pool) = &gate {
            pool.borrow_mut().deliver(driven);
        }
    });
    task.borrow_mut().driver = Some(driver);

    registry.borrow_mut().tasks.insert(id, Rc::clone(&task));
    Ok(task)
}

// How many pooled task bodies are on the stack, for the reentrancy check.
//
// A counter rather than a flag because pooled work may nest through
// `task.spawn`, and the check is "is any pool's task below me", not "is exactly
// one".
//
// Thread-local is sound and not a shortcut: the runtime is current-thread by
// construction (see the module docs), so this counts the one stack there is.
thread_local! {
    static IN_POOL: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Raises [`IN_POOL`] for as long as it lives.
///
/// A guard rather than a bare increment/decrement pair so an early return out of
/// the driver cannot leak the count and poison every later `await`.
struct PoolDepth;

impl PoolDepth {
    fn enter() -> Self {
        IN_POOL.set(IN_POOL.get() + 1);
        Self
    }
}

impl Drop for PoolDepth {
    fn drop(&mut self) {
        IN_POOL.set(IN_POOL.get() - 1);
    }
}

/// Wraps a pooled task's body so [`IN_POOL`] is raised across each poll of it
/// and lowered again as soon as it parks.
///
/// The distinction matters because every task shares one thread: a pooled task
/// waiting on `task.wait` is *not* on the stack, so the count must not be raised
/// then, or an ordinary `pool:await` in the main script would be mistaken for
/// reentrant pooled work. Scoping it to the poll makes the check ask exactly
/// "am I called from within pooled work", which is the deadlock condition.
struct InPool<F>(F);

impl<F: std::future::Future> std::future::Future for InPool<F> {
    type Output = F::Output;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<F::Output> {
        let _depth = PoolDepth::enter();
        // Safe: we never move out of the projected field, and `InPool` adds no
        // `Unpin` guarantee of its own.
        let inner = unsafe { self.map_unchecked_mut(|s| &mut s.0) };
        inner.poll(cx)
    }
}

/// Raise if the caller is running inside a pooled task.
///
/// A pooled task holds its permit while parked, so a pool awaited from within
/// pooled work deadlocks once `concurrency` tasks are doing it -- silently, with
/// no deadline to break it. Raising turns an unrecoverable hang into a script
/// error that names the mistake.
fn reject_reentrant(what: &str) -> LuaResult<()> {
    if IN_POOL.get() > 0 {
        return Err(mlua::Error::runtime(format!(
            "{what} cannot be called from inside a pooled task: the caller holds \
             a permit while it waits, so the pool would deadlock. Collect results \
             from outside the pool, or use task.spawn/task.await for nested work."
        )));
    }
    Ok(())
}

/// One pool: a permit gate plus a completion queue.
struct Pool {
    /// Bounds how many task *bodies* run at once. Acquired inside the driver,
    /// before the thread is first resumed, so `spawn` never blocks and a queued
    /// task has executed no script code.
    permits: Rc<tokio::sync::Semaphore>,
    /// Tasks that have settled and not yet been collected, in completion order.
    ready: std::collections::VecDeque<Rc<RefCell<Task>>>,
    /// Spawned but not yet collected -- queued, running and ready alike. This is
    /// what `count`/`done` report: how many more `await`s can produce something.
    outstanding: usize,
    /// Every task this pool has spawned that has not been collected, so
    /// `cancel` can reach the queued and running ones too.
    live: Vec<Rc<RefCell<Task>>>,
    /// The concurrency the pool was created with.
    concurrency: usize,
    /// Wakers of `await`s parked on this pool, woken when a task settles.
    waiters: Vec<std::task::Waker>,
}

impl Pool {
    /// Record a settled task and release anyone waiting for a result.
    fn deliver(&mut self, task: Rc<RefCell<Task>>) {
        self.ready.push_back(task);
        for waker in self.waiters.drain(..) {
            waker.wake();
        }
    }
}

/// A Lua-side handle to a pool. Cloneable, so a pool can be passed around.
///
/// The methods are installed by [`pool_methods`] rather than an `impl UserData`,
/// because they need the task registry captured -- a pooled task has to land in
/// it for the drain to cover it.
#[derive(Clone)]
struct PoolHandle(Rc<RefCell<Pool>>);

/// A future that completes once the pool has a collectable result.
struct Ready(Rc<RefCell<Pool>>);

impl std::future::Future for Ready {
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        let mut pool = self.0.borrow_mut();
        // Also done when nothing is outstanding (a cancel emptied the pool), so
        // the caller raises "drained" instead of parking forever.
        if pool.ready.is_empty() && pool.outstanding > 0 {
            let waker = cx.waker();
            if !pool.waiters.iter().any(|w| w.will_wake(waker)) {
                pool.waiters.push(waker.clone());
            }
            return std::task::Poll::Pending;
        }
        std::task::Poll::Ready(())
    }
}

/// Take the next settled task out of the pool, or raise if there is none left
/// to wait for.
///
/// Shared by `await` and `poll` so the drained-pool wording and the outstanding
/// bookkeeping cannot drift between them.
fn collect(pool: &Rc<RefCell<Pool>>, what: &str) -> LuaResult<MultiValue> {
    let task = {
        let mut p = pool.borrow_mut();
        let Some(task) = p.ready.pop_front() else {
            return Err(mlua::Error::runtime(format!(
                "{what} on a drained pool: every task has been collected. \
                 Guard the loop with pool:done() or pool:count()."
            )));
        };
        p.outstanding -= 1;
        // No longer this pool's to cancel -- it has already settled, and a
        // collected result is the script's.
        let id = task.borrow().id;
        p.live.retain(|t| t.borrow().id != id);
        task
    };
    take_settled(&task)
}

/// Cancel a pending task, as `task.cancel` does.
///
/// Used by `task.timeout` when the deadline wins. Woken, so anything awaiting
/// the handle stops waiting rather than hanging until the drain.
fn cancel_task(task: &Rc<RefCell<Task>>) -> bool {
    let mut task = task.borrow_mut();
    // Only a pending task can be cancelled: a finished one keeps its result,
    // because cancelling cannot undo work already done.
    if !matches!(task.state, State::Pending) {
        return false;
    }
    if let Some(driver) = &task.driver {
        driver.abort();
    }
    task.settle(State::Cancelled);
    true
}

/// Install the pool methods.
///
/// `registry` is captured so a pooled task is an ordinary registry task: the
/// drain covers it, which is what makes an uncollected failure fail the run.
fn pool_methods<M: UserDataMethods<PoolHandle>>(methods: &mut M, registry: &Rc<RefCell<Registry>>) {
    let reg = Rc::clone(registry);
    methods.add_method("spawn", move |lua, this, args: Variadic<Value>| {
        let mut args = args.into_iter();
        let f = match args.next() {
            Some(Value::Function(f)) => f,
            other => {
                return Err(mlua::Error::runtime(format!(
                    "pool:spawn expects a function, got {}",
                    other.as_ref().map_or("no value", Value::type_name)
                )));
            }
        };
        let rest = MultiValue::from_iter(args);
        let task = spawn_task(lua, &reg, f, rest, Some(Rc::clone(&this.0)))?;
        {
            let mut pool = this.0.borrow_mut();
            pool.outstanding += 1;
            pool.live.push(task);
        }
        // No handle: the pool owns the result, and `await`/`poll` are the only
        // way to it. One collection path, so nothing has to agree about who
        // claimed a failure.
        Ok(())
    });

    methods.add_async_method("await", |_, this, ()| async move {
        reject_reentrant("pool:await")?;
        // Check for a drained pool before parking, or an empty pool would hang
        // forever instead of raising.
        if this.0.borrow().outstanding == 0 {
            return collect(&this.0, "pool:await");
        }
        Ready(Rc::clone(&this.0)).await;
        collect(&this.0, "pool:await")
    });

    methods.add_method("poll", |_, this, ()| {
        reject_reentrant("pool:poll")?;
        if this.0.borrow().outstanding == 0 {
            return Err(mlua::Error::runtime(
                "pool:poll on a drained pool: every task has been collected. \
                 Guard the loop with pool:done() or pool:count()."
                    .to_string(),
            ));
        }
        if this.0.borrow().ready.is_empty() {
            return Ok(MultiValue::from_iter([Value::Boolean(false)]));
        }
        let mut out = MultiValue::from_iter([Value::Boolean(true)]);
        for v in collect(&this.0, "pool:poll")? {
            out.push_back(v);
        }
        Ok(out)
    });

    // `done` and `count` are one fact read two ways, so they share the number
    // rather than tracking it separately.
    methods.add_method("count", |_, this, ()| Ok(this.0.borrow().outstanding));
    methods.add_method("done", |_, this, ()| Ok(this.0.borrow().outstanding == 0));

    methods.add_method("concurrency", |_, this, ()| Ok(this.0.borrow().concurrency));

    methods.add_meta_method(mlua::MetaMethod::ToString, |_, this, ()| {
        let pool = this.0.borrow();
        Ok(format!(
            "task.pool({}) ({} outstanding)",
            pool.concurrency, pool.outstanding
        ))
    });

    // Returns how many tasks were still pending and got cancelled; ones that
    // had already settled (collected or not) are not counted.
    methods.add_method("cancel", |_, this, ()| {
        // Take the lists out before cancelling: `cancel_task` wakes waiters, and
        // holding the pool's borrow across that risks a borrow panic if a woken
        // task reaches back into the pool.
        let (live, ready, waiters) = {
            let mut pool = this.0.borrow_mut();
            pool.outstanding = 0;
            (
                std::mem::take(&mut pool.live),
                std::mem::take(&mut pool.ready),
                std::mem::take(&mut pool.waiters),
            )
        };
        let mut cancelled = 0usize;
        for task in &live {
            if cancel_task(task) {
                cancelled += 1;
            }
        }
        // A failure nobody can collect now is part of the abandoned sweep, so
        // it must not fail the run from the drain either.
        for task in live.iter().chain(ready.iter()) {
            if let State::Failed { reported, .. } = &mut task.borrow_mut().state {
                *reported = true;
            }
        }
        // Wake parked `pool:await`s: nothing is outstanding, so they raise
        // "drained" instead of hanging.
        for waker in waiters {
            waker.wake();
        }
        // Cancellation is exempt from drain reporting, so this is the only way
        // to abandon a sweep without failing the run.
        Ok(cancelled)
    });
}

/// Build the `task` module table.
///
/// The returned [`Tasks`] is the host's drain handle; it must be awaited after
/// the script returns and before the VFS is flushed.
pub fn module(lua: &Lua) -> LuaResult<(Table, Tasks)> {
    let exports = lua.create_table()?;
    let registry: Rc<RefCell<Registry>> = Rc::new(RefCell::new(Registry::default()));

    // --- spawn ------------------------------------------------------------
    let reg = Rc::clone(&registry);
    exports.raw_set(
        "spawn",
        lua.create_function(move |lua, args: Variadic<Value>| {
            let mut args = args.into_iter();
            let f = match args.next() {
                Some(Value::Function(f)) => f,
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "task.spawn expects a function, got {}",
                        other.as_ref().map_or("no value", Value::type_name)
                    )));
                }
            };
            let rest = MultiValue::from_iter(args);
            Ok(Handle(spawn_task(lua, &reg, f, rest, None)?))
        })?,
    )?;

    // --- await ------------------------------------------------------------
    exports.raw_set(
        "await",
        lua.create_async_function(|_, args: Variadic<Value>| async move {
            let handles = handles_from(&args, "task.await")?;

            // Settle *every* handle before looking at any result. Awaiting in
            // list order costs nothing -- the tasks are all being driven by the
            // runtime already, so the ones later in the list keep progressing
            // while this one is parked -- but bailing out on the first failure
            // would leave the rest of the list unawaited, and an unawaited
            // failure is one the drain reports and fails the run with, outside
            // any `pcall` the script wrapped this call in. Waiting for all of
            // them is what makes "an error surfaces at await, catchable with
            // pcall" true for a batch and not just for a single handle.
            for task in &handles {
                Settled(Rc::clone(task)).await;
            }

            // Now that nothing is pending, read the results. `take_settled`
            // marks a failure reported, so this pass also claims the errors of
            // the handles *after* the first failing one -- they were the
            // script's to see, and the drain must not repeat them.
            let mut out = MultiValue::new();
            let mut first_error = None;
            for (i, task) in handles.iter().enumerate() {
                let values = match take_settled(task) {
                    Ok(values) => values,
                    Err(e) => {
                        // With several handles, say which one failed.
                        let e = if handles.len() > 1 {
                            mlua::Error::runtime(format!(
                                "task.await: handle {} (task#{}) failed: {e}",
                                i + 1,
                                task.borrow().id
                            ))
                        } else {
                            e
                        };
                        first_error.get_or_insert(e);
                        MultiValue::new()
                    }
                };
                // One handle in, its results out unchanged, so a single-task
                // await reads like a plain call. Several handles in, one value
                // per handle positionally -- flattening multi-value returns
                // would make the positions stop lining up with the handles.
                if handles.len() == 1 && first_error.is_none() {
                    return Ok(values);
                }
                out.push_back(values.into_iter().next().unwrap_or(Value::Nil));
            }
            match first_error {
                Some(e) => Err(e),
                None => Ok(out),
            }
        })?,
    )?;

    // --- poll -------------------------------------------------------------
    exports.raw_set(
        "poll",
        lua.create_function(|_, handle: Value| {
            let task = handle_from(&handle, "task.poll")?;
            if matches!(task.borrow().state, State::Pending) {
                return Ok(MultiValue::from_iter([Value::Boolean(false)]));
            }
            let mut out = MultiValue::from_iter([Value::Boolean(true)]);
            for v in take_settled(&task)? {
                out.push_back(v);
            }
            Ok(out)
        })?,
    )?;

    // --- status -----------------------------------------------------------
    exports.raw_set(
        "status",
        lua.create_function(|_, handle: Value| {
            let task = handle_from(&handle, "task.status")?;
            let state = match task.borrow().state {
                State::Pending => "pending",
                State::Done(..) => "done",
                State::Failed { .. } => "failed",
                State::Cancelled => "cancelled",
            };
            Ok(state)
        })?,
    )?;

    // --- cancel -----------------------------------------------------------
    exports.raw_set(
        "cancel",
        lua.create_function(|_, handle: Value| {
            Ok(cancel_task(&handle_from(&handle, "task.cancel")?))
        })?,
    )?;

    // --- wait -------------------------------------------------------------
    exports.raw_set(
        "wait",
        lua.create_async_function(|_, duration: Value| async move {
            tokio::time::sleep(duration_from(&duration, "task.wait")?).await;
            Ok(())
        })?,
    )?;

    // --- TIMEOUT ----------------------------------------------------------
    // One instance, created once and shared, because identity is the whole
    // point: every `task.timeout` expiry must return the *same* value for
    // `== task.TIMEOUT` to work.
    let sentinel = lua.create_userdata(Timeout)?;
    exports.raw_set("TIMEOUT", sentinel.clone())?;

    // --- timeout ----------------------------------------------------------
    let reg = Rc::clone(&registry);
    exports.raw_set(
        "timeout",
        lua.create_async_function(move |lua, args: Variadic<Value>| {
            let reg = Rc::clone(&reg);
            let sentinel = sentinel.clone();
            async move {
                let mut args = args.into_iter();
                let duration = duration_from(&args.next().unwrap_or(Value::Nil), "task.timeout")?;
                let f = match args.next() {
                    Some(Value::Function(f)) => f,
                    other => {
                        return Err(mlua::Error::runtime(format!(
                            "task.timeout expects a function after the duration, got {}",
                            other.as_ref().map_or("no value", Value::type_name)
                        )));
                    }
                };
                let rest = MultiValue::from_iter(args);

                // An ordinary task, so it is drained, cancellable and reported
                // like any other -- the deadline is the only thing added.
                let task = spawn_task(&lua, &reg, f, rest, None)?;

                // Race the task against the clock. `Settled` parks on the
                // task's waker list, so losing this race costs the task
                // nothing: it is driven by its own `spawn_local` either way.
                let settled = Settled(Rc::clone(&task));
                if tokio::time::timeout(duration, settled).await.is_err() {
                    // The deadline won. Cancel, so the abandoned work stops
                    // where it can and the drain does not wait for it.
                    cancel_task(&task);
                    return Ok(MultiValue::from_iter([Value::UserData(sentinel)]));
                }

                // The task won. Its values come back unchanged, so a timed call
                // reads like a plain one; an error it raised propagates here,
                // because only the deadline yields the sentinel.
                take_settled(&task)
            }
        })?,
    )?;

    // --- pool -------------------------------------------------------------
    // The methods need the registry, so the type is registered here with it
    // captured rather than through a plain `impl UserData`.
    let reg = Rc::clone(&registry);
    lua.register_userdata_type::<PoolHandle>(move |methods| pool_methods(methods, &reg))?;

    exports.raw_set(
        "pool",
        lua.create_function(move |lua, concurrency: Value| {
            let max = tokio::sync::Semaphore::MAX_PERMITS;
            let bad = |shown: String| {
                mlua::Error::runtime(format!(
                    "task.pool expects a whole concurrency between 1 and {max}, got {shown}"
                ))
            };
            let n = match &concurrency {
                Value::Integer(n) => *n as f64,
                Value::Number(n) if n.fract() == 0.0 => *n,
                Value::Number(n) => return Err(bad(n.to_string())),
                other => return Err(bad(other.type_name().to_string())),
            };
            if !(n >= 1.0 && n <= max as f64) {
                return Err(bad(n.to_string()));
            }
            let permits = n as usize;
            let pool = Rc::new(RefCell::new(Pool {
                permits: Rc::new(tokio::sync::Semaphore::new(permits)),
                ready: std::collections::VecDeque::new(),
                outstanding: 0,
                live: Vec::new(),
                concurrency: permits,
                waiters: Vec::new(),
            }));
            lua.create_any_userdata(PoolHandle(pool))
        })?,
    )?;

    Ok((exports, Tasks { registry }))
}

/// Install the `task` module as a global.
pub fn register(lua: &Lua) -> LuaResult<Tasks> {
    let (module, tasks) = module(lua)?;
    lua.globals().raw_set("task", module)?;
    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `script`, then drain as the host does, on a current-thread runtime
    /// with a `LocalSet` -- the same shape as `main`, since `spawn_local` needs
    /// one and the whole point is that tasks are not `Send`.
    fn eval(script: &str) -> LuaResult<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build test runtime");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let lua = Lua::new();
            let tasks = register(&lua)?;
            lua.load(script).exec_async().await?;
            tasks.drain().await
        })
    }

    /// Wall-clock duration of `f`, for the overlap assertions.
    fn timed(f: impl FnOnce()) -> Duration {
        let start = std::time::Instant::now();
        f();
        start.elapsed()
    }

    #[test]
    fn concurrent_waits_overlap() {
        // The premise of the whole module: four sleeps that would take 2s
        // serially finish in about one sleep's worth of wall clock.
        let elapsed = timed(|| {
            eval(
                r#"
                local hs = {}
                for i = 1, 4 do
                    hs[i] = task.spawn(function() task.wait(0.5) return i end)
                end
                local a, b, c, d = task.await(hs)
                assert(a == 1 and b == 2 and c == 3 and d == 4, "results out of order")
            "#,
            )
            .unwrap();
        });
        assert!(
            elapsed < Duration::from_millis(1200),
            "expected overlap, took {elapsed:?}"
        );
    }

    #[test]
    fn await_accepts_handles_as_arguments() {
        eval(
            r#"
            local a = task.spawn(function() task.wait(0.05) return "a" end)
            local b = task.spawn(function() return "b" end)
            local ra, rb = task.await(a, b)
            assert(ra == "a" and rb == "b", "unexpected results")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn single_await_returns_all_values() {
        // One handle is the "reads like a plain call" case, so a multi-value
        // return survives intact rather than being truncated to the first.
        eval(
            r#"
            local h = task.spawn(function() return 1, 2, 3 end)
            local a, b, c = task.await(h)
            assert(a == 1 and b == 2 and c == 3, "expected all three values")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn spawn_passes_arguments_through() {
        eval(
            r#"
            local h = task.spawn(function(x, y) return x + y end, 2, 3)
            assert(task.await(h) == 5, "expected arguments to reach the body")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn poll_is_non_blocking_and_non_destructive() {
        eval(
            r#"
            local h = task.spawn(function() task.wait(0.2) return "v" end)
            assert(task.poll(h) == false, "a just-spawned task should be pending")

            task.wait(0.4)
            local ready, value = task.poll(h)
            assert(ready and value == "v", "expected the result after waiting")

            -- Polling does not consume, so both a second poll and a later
            -- await still see the value.
            local ready2, value2 = task.poll(h)
            assert(ready2 and value2 == "v", "poll should not consume the result")
            assert(task.await(h) == "v", "await after poll should still work")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn cancel_stops_a_pending_task() {
        eval(r#"
            local h = task.spawn(function() task.wait(30) return "never" end)
            assert(task.cancel(h) == true, "cancelling a pending task should report true")
            -- Already settled: cancelling again changes nothing.
            assert(task.cancel(h) == false, "cancelling twice should report false")
            local ok, err = pcall(function() return task.await(h) end)
            assert(not ok, "awaiting a cancelled task should fail")
            assert(string.find(tostring(err), "cancelled") ~= nil, "expected a cancellation error, got " .. tostring(err))
        "#)
        .unwrap();
    }

    #[test]
    fn cancel_does_not_stall_the_drain() {
        // A cancelled task must not be waited for, or this would take 30s.
        let elapsed = timed(|| {
            eval(
                r#"
                local h = task.spawn(function() task.wait(30) end)
                task.cancel(h)
            "#,
            )
            .unwrap();
        });
        assert!(
            elapsed < Duration::from_secs(1),
            "drain waited for a cancelled task, took {elapsed:?}"
        );
    }

    #[test]
    fn cancel_after_completion_keeps_the_result() {
        eval(
            r#"
            local h = task.spawn(function() return "done" end)
            task.wait(0.1)
            assert(task.cancel(h) == false, "a finished task cannot be cancelled")
            assert(task.await(h) == "done", "the result should survive a late cancel")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn error_surfaces_at_await() {
        eval(r#"
            local h = task.spawn(function() error("boom") end)
            local ok, err = pcall(function() return task.await(h) end)
            assert(not ok, "expected the await to fail")
            assert(string.find(tostring(err), "boom") ~= nil, "expected the task's message, got " .. tostring(err))
        "#)
        .unwrap();
    }

    #[test]
    fn await_settles_every_handle_before_raising() {
        // The failure mode this guards: `await` used to return at the first
        // failing handle, leaving the rest of the batch unawaited. Those stayed
        // unreported, so the drain failed the run even though the script had
        // wrapped the await in `pcall` -- one bad task in a batch was
        // uncatchable however carefully you guarded it.
        eval(
            r#"
            local hs = {}
            for i = 1, 5 do
                hs[i] = task.spawn(function()
                    task.wait(0.01 * i)
                    if i >= 2 then error("boom " .. i) end
                    return i
                end)
            end
            local ok, err = pcall(function() return task.await(hs) end)
            assert(not ok, "expected the await to fail")
            -- The first failure in list order is the one raised.
            assert(string.find(tostring(err), "boom 2") ~= nil,
                "expected the first failure, got " .. tostring(err))
        "#,
        )
        .expect("the drain must have nothing left to report");
    }

    #[test]
    fn await_reports_a_later_failure_after_an_earlier_one() {
        // Same guarantee from the other side: even when the *first* handle is
        // the one that fails, the siblings behind it are still claimed, so the
        // drain stays quiet.
        eval(
            r#"
            local a = task.spawn(function() error("first") end)
            local b = task.spawn(function() task.wait(0.05) error("second") end)
            local ok = pcall(function() return task.await(a, b) end)
            assert(not ok, "expected the await to fail")
        "#,
        )
        .expect("the drain must have nothing left to report");
    }

    #[test]
    fn awaited_failure_does_not_fail_the_run() {
        // The script saw the error and chose to handle it, so the drain has
        // nothing left to report.
        eval(
            r#"
            local h = task.spawn(function() error("handled") end)
            pcall(function() return task.await(h) end)
        "#,
        )
        .unwrap();
    }

    #[test]
    fn unawaited_failure_fails_the_run() {
        // Nobody looked, so the drain is the only place this can surface.
        let err = eval(r#"task.spawn(function() error("unseen") end)"#)
            .expect_err("expected the drain to report the failure");
        assert!(
            err.to_string().contains("unseen"),
            "expected the task's message, got {err}"
        );
    }

    #[test]
    fn drain_waits_for_an_unawaited_task() {
        // The host relies on this: a task's buffered writes are only real once
        // it has finished, and the flush happens after the drain.
        eval(
            r#"
            _G.ran = false
            task.spawn(function() task.wait(0.2) _G.ran = true end)
            assert(_G.ran == false, "the task should not have finished yet")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn a_task_can_spawn_and_await_another() {
        eval(
            r#"
            local outer = task.spawn(function()
                local inner = task.spawn(function() task.wait(0.05) return 7 end)
                return task.await(inner) + 1
            end)
            assert(task.await(outer) == 8, "expected nested tasks to compose")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn several_tasks_can_await_one_handle() {
        // Waiters are a list, not a single slot, so a shared handle wakes all
        // of them rather than losing every waker but the last.
        eval(
            r#"
            local shared = task.spawn(function() task.wait(0.2) return "s" end)
            local a = task.spawn(function() return task.await(shared) .. "1" end)
            local b = task.spawn(function() return task.await(shared) .. "2" end)
            local ra, rb = task.await(a, b)
            assert(ra == "s1" and rb == "s2", "expected both waiters to be woken")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn wait_rejects_nonsense_durations() {
        for bad in ["-1", "0/0", "1/0", "'soon'", "'5 fortnights'", "{}", "nil"] {
            let script = format!("task.wait({bad})");
            assert!(
                eval(&script).is_err(),
                "task.wait({bad}) should have been rejected"
            );
        }
    }

    #[test]
    fn wait_accepts_humantime_strings() {
        // The numeric form predates strings and must keep working alongside
        // them, since every existing script uses it.
        eval(r#"task.wait("10ms") task.wait(0.01) task.wait("1ms 500us")"#).unwrap();
    }

    #[test]
    fn timeout_returns_the_function_result() {
        eval(
            r#"
            local r = task.timeout("10s", function() return "value" end)
            assert(r == "value", "expected the function's own return, got " .. tostring(r))
            assert(r ~= task.TIMEOUT, "a fast function must not report a timeout")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn timeout_returns_all_values_unchanged() {
        // Same rule as a single-handle await: a timed call reads like a plain
        // one, so a multi-value return survives intact.
        eval(
            r#"
            local a, b, c = task.timeout("10s", function() return 1, 2, 3 end)
            assert(a == 1 and b == 2 and c == 3, "expected all three values")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn timeout_yields_the_sentinel_when_the_deadline_passes() {
        eval(
            r#"
            local r = task.timeout("50ms", function() task.wait(30) return "never" end)
            assert(r == task.TIMEOUT, "expected the sentinel, got " .. tostring(r))
            assert(tostring(task.TIMEOUT) == "task.TIMEOUT", "sentinel should name itself")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn timeout_does_not_stall_the_drain() {
        // Expiry cancels, so the run must not then wait out the full 30s the
        // abandoned task asked for.
        let elapsed = timed(|| {
            eval(r#"task.timeout("50ms", function() task.wait(30) end)"#).unwrap();
        });
        assert!(
            elapsed < Duration::from_secs(1),
            "drain waited for a timed-out task, took {elapsed:?}"
        );
    }

    #[test]
    fn timeout_passes_arguments_through() {
        eval(
            r#"
            local r = task.timeout("10s", function(x, y) return x + y end, 2, 3)
            assert(r == 5, "expected arguments to reach the body")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn timeout_propagates_an_error_rather_than_the_sentinel() {
        // The sentinel means "too slow" and nothing else, so a function that
        // raised must raise here -- catchable, and distinguishable from expiry.
        eval(
            r#"
            local ok, err = pcall(function()
                return task.timeout("10s", function() error("boom") end)
            end)
            assert(not ok, "expected the error to propagate")
            assert(string.find(tostring(err), "boom") ~= nil,
                "expected the function's message, got " .. tostring(err))
        "#,
        )
        .expect("a caught error leaves the drain nothing to report");
    }

    #[test]
    fn timeout_accepts_seconds_as_a_number_too() {
        eval(
            r#"
            assert(task.timeout(10, function() return "v" end) == "v")
            assert(task.timeout(0.05, function() task.wait(30) end) == task.TIMEOUT)
        "#,
        )
        .unwrap();
    }

    #[test]
    fn timeout_rejects_bad_arguments() {
        assert!(eval(r#"task.timeout("10s", 42)"#).is_err());
        assert!(eval(r#"task.timeout("10s")"#).is_err());
        assert!(eval(r#"task.timeout("nonsense", function() end)"#).is_err());
        assert!(eval(r#"task.timeout(function() end)"#).is_err());
        assert!(eval("task.timeout()").is_err());
    }

    #[test]
    fn timeout_runs_concurrently_with_other_tasks() {
        // A timed block is an ordinary task, so it must not pin the thread:
        // three 300ms timed blocks under one deadline overlap.
        let elapsed = timed(|| {
            eval(
                r#"
                local hs = {}
                for i = 1, 3 do
                    hs[i] = task.spawn(function()
                        return task.timeout("10s", function()
                            task.wait(0.3)
                            return i
                        end)
                    end)
                end
                local a, b, c = task.await(hs)
                assert(a == 1 and b == 2 and c == 3, "results out of order")
            "#,
            )
            .unwrap();
        });
        assert!(
            elapsed < Duration::from_millis(800),
            "expected overlap, took {elapsed:?}"
        );
    }

    #[test]
    fn a_timed_out_task_is_not_reported_by_the_drain() {
        // Cancellation is the mechanism, and a cancelled task is neither driven
        // to completion nor reported -- so an expiry the script *handled* by
        // reading the sentinel must not also fail the run.
        eval(
            r#"
            local r = task.timeout("20ms", function()
                task.wait(5)
                error("should never be reached")
            end)
            assert(r == task.TIMEOUT, "expected the sentinel")
        "#,
        )
        .expect("a timed-out task must not fail the run");
    }

    #[test]
    fn pool_bounds_how_many_run_at_once() {
        // The premise: six 200ms sleeps through a pool of two take three
        // rounds, not one. Serial would be 1.2s, unbounded ~0.2s.
        let elapsed = timed(|| {
            eval(
                r#"
                local pool = task.pool(2)
                for i = 1, 6 do
                    pool:spawn(function() task.wait(0.2) return i end)
                end
                local n = 0
                while not pool:done() do
                    pool:await()
                    n = n + 1
                end
                assert(n == 6, "expected six results, got " .. n)
            "#,
            )
            .unwrap();
        });
        assert!(
            elapsed >= Duration::from_millis(550),
            "pool did not bound concurrency, took {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_millis(1100),
            "pool serialized instead of running two at a time, took {elapsed:?}"
        );
    }

    #[test]
    fn pool_yields_results_in_completion_order() {
        // Not spawn order: the point of a pool over `task.await(list)` is that a
        // slow task does not hold up the ones behind it.
        eval(
            r#"
            local pool = task.pool(4)
            for _, d in ipairs({ 0.3, 0.05, 0.2, 0.1 }) do
                pool:spawn(function() task.wait(d) return d end)
            end
            local got = {}
            while not pool:done() do
                table.insert(got, pool:await())
            end
            assert(#got == 4, "expected four results")
            assert(got[1] == 0.05 and got[2] == 0.1 and got[3] == 0.2 and got[4] == 0.3,
                "results not in completion order: " .. table.concat(got, ","))
        "#,
        )
        .unwrap();
    }

    #[test]
    fn pool_spawn_returns_nothing() {
        // The pool owns the result, so there is no handle to bypass it with.
        eval(
            r#"
            local pool = task.pool(1)
            local h = pool:spawn(function() return "v" end)
            assert(h == nil, "pool:spawn must not hand back a handle")
            assert(pool:await() == "v")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn pool_await_returns_all_values() {
        // Same rule as a single-handle await: a task's values come back
        // unchanged, which is what lets a task return its own identity.
        eval(
            r#"
            local pool = task.pool(2)
            pool:spawn(function() return "host-a", 1, 2 end)
            local host, a, b = pool:await()
            assert(host == "host-a" and a == 1 and b == 2, "expected all three values")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn pool_spawn_passes_arguments_through() {
        eval(
            r#"
            local pool = task.pool(2)
            pool:spawn(function(x, y) return x + y end, 2, 3)
            assert(pool:await() == 5, "expected arguments to reach the body")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn pool_count_and_done_track_collection() {
        eval(
            r#"
            local pool = task.pool(2)
            assert(pool:done() == true, "an empty pool is done")
            assert(pool:count() == 0)
            for i = 1, 3 do pool:spawn(function() return i end) end
            assert(pool:count() == 3, "count is spawned-but-uncollected")
            assert(pool:done() == false)
            pool:await()
            assert(pool:count() == 2, "collecting decrements")
            pool:await() pool:await()
            assert(pool:count() == 0 and pool:done() == true, "drained")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn pool_poll_is_non_blocking() {
        eval(
            r#"
            local pool = task.pool(2)
            pool:spawn(function() task.wait(0.2) return "v" end)
            assert(pool:poll() == false, "nothing should be ready yet")
            task.wait(0.4)
            local ready, value = pool:poll()
            assert(ready and value == "v", "expected the result after waiting")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn pool_await_and_poll_raise_on_a_drained_pool() {
        // Not `nil`: a task may legitimately return `nil` first, and the two
        // must stay distinguishable -- the ambiguity `task.TIMEOUT` avoids.
        eval(
            r#"
            local pool = task.pool(1)
            pool:spawn(function() return nil end)
            local first = pool:await()
            assert(first == nil, "a task returning nil is not a drained pool")
            assert(pool:done() == true)

            local ok, err = pcall(function() return pool:await() end)
            assert(not ok, "awaiting a drained pool should raise")
            assert(string.find(tostring(err), "drained") ~= nil,
                "expected a drained-pool error, got " .. tostring(err))

            local ok2 = pcall(function() return pool:poll() end)
            assert(not ok2, "polling a drained pool should raise")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn pool_await_reraises_a_task_error() {
        // Same convention as `task.await`: errors raise, caught with `pcall`.
        eval(
            r#"
            local pool = task.pool(2)
            pool:spawn(function() error("boom") end)
            local ok, err = pcall(function() return pool:await() end)
            assert(not ok, "expected the await to fail")
            assert(string.find(tostring(err), "boom") ~= nil,
                "expected the task's message, got " .. tostring(err))
            assert(pool:done() == true, "a raised result is still collected")
        "#,
        )
        .expect("a collected error leaves the drain nothing to report");
    }

    #[test]
    fn pool_failure_survives_one_bad_task_in_a_sweep() {
        // The pattern a pool exists for: three bad hosts out of many must not
        // abort the sweep, which `task.await(list)` cannot offer.
        eval(
            r#"
            local pool = task.pool(3)
            for i = 1, 6 do
                pool:spawn(function()
                    if i % 2 == 0 then error("bad " .. i) end
                    return i
                end)
            end
            local good, bad = 0, 0
            while not pool:done() do
                local ok = pcall(function() return pool:await() end)
                if ok then good = good + 1 else bad = bad + 1 end
            end
            assert(good == 3 and bad == 3, "expected 3/3, got " .. good .. "/" .. bad)
        "#,
        )
        .expect("every failure was collected, so the drain stays quiet");
    }

    #[test]
    fn uncollected_pool_failure_fails_the_run() {
        // A pooled task is an ordinary task: nobody looked, so the drain is the
        // only place this can surface. Otherwise `pool:spawn` would be a way to
        // swallow errors.
        let err = eval(
            r#"
            local pool = task.pool(2)
            pool:spawn(function() error("unseen") end)
        "#,
        )
        .expect_err("expected the drain to report the failure");
        assert!(
            err.to_string().contains("unseen"),
            "expected the task's message, got {err}"
        );
    }

    #[test]
    fn pool_drain_completes_uncollected_tasks() {
        // Buffered writes from a pooled task must still land, so the drain waits
        // for work nobody collected.
        eval(
            r#"
            _G.ran = 0
            local pool = task.pool(2)
            for i = 1, 4 do
                pool:spawn(function() task.wait(0.05) _G.ran = _G.ran + 1 end)
            end
            assert(_G.ran == 0, "nothing should have finished yet")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn pool_cancel_abandons_the_sweep_without_failing_the_run() {
        // Cancellation is exempt from drain reporting, so this is the only way
        // to stop early -- including for tasks that would have failed.
        let elapsed = timed(|| {
            eval(
                r#"
                local pool = task.pool(2)
                for i = 1, 6 do
                    pool:spawn(function() task.wait(30) error("never seen") end)
                end
                local n = pool:cancel()
                assert(n > 0, "expected cancel to report the tasks it stopped")
                assert(pool:done() == true, "a cancelled pool is drained")
            "#,
            )
            .expect("a cancelled pool must not fail the run");
        });
        assert!(
            elapsed < Duration::from_secs(1),
            "drain waited for cancelled pool tasks, took {elapsed:?}"
        );
    }

    #[test]
    fn pool_rejects_awaiting_from_inside_pooled_work() {
        // The deadlock this prevents: a pooled task holds its permit while
        // parked, so `concurrency` tasks awaiting a pool wedge it with no error
        // and no deadline to break it.
        eval(
            r#"
            local pool = task.pool(2)
            local inner = task.pool(2)
            inner:spawn(function() return "x" end)
            pool:spawn(function()
                local ok, err = pcall(function() return inner:await() end)
                assert(not ok, "awaiting any pool from pooled work should raise")
                assert(string.find(tostring(err), "pooled task") ~= nil,
                    "expected a reentrancy error, got " .. tostring(err))
                local ok2 = pcall(function() return inner:poll() end)
                assert(not ok2, "poll should refuse too")
                return true
            end)
            assert(pool:await() == true)
            inner:cancel()
        "#,
        )
        .unwrap();
    }

    #[test]
    fn pool_permits_are_released_by_a_failing_task() {
        // A body that raises must still give its permit back, or a pool of one
        // wedges after the first failure.
        eval(
            r#"
            local pool = task.pool(1)
            for i = 1, 3 do
                pool:spawn(function() error("boom " .. i) end)
            end
            local seen = 0
            while not pool:done() do
                pcall(function() return pool:await() end)
                seen = seen + 1
            end
            assert(seen == 3, "expected all three to run, got " .. seen)
        "#,
        )
        .expect("every failure was collected");
    }

    #[test]
    fn pool_task_may_spawn_ordinary_tasks() {
        // Nested `task.spawn`/`task.await` stays available inside pooled work --
        // only pools are refused, so this is the documented escape.
        eval(
            r#"
            local pool = task.pool(2)
            pool:spawn(function()
                local h = task.spawn(function() task.wait(0.05) return 7 end)
                return task.await(h) + 1
            end)
            assert(pool:await() == 8, "expected nested task.spawn to compose")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn pool_rejects_a_bad_concurrency() {
        for bad in ["0", "-1"] {
            assert!(
                eval(&format!("task.pool({bad})")).is_err(),
                "task.pool({bad}) should have been rejected"
            );
        }
    }

    #[test]
    fn pool_spawn_rejects_a_non_function() {
        assert!(eval("task.pool(1):spawn(42)").is_err());
        assert!(eval("task.pool(1):spawn()").is_err());
    }

    #[test]
    fn spawn_rejects_a_non_function() {
        assert!(eval("task.spawn(42)").is_err());
        assert!(eval("task.spawn()").is_err());
    }

    #[test]
    fn await_and_poll_reject_a_non_handle() {
        assert!(eval("task.await(42)").is_err());
        assert!(eval("task.poll('nope')").is_err());
        assert!(eval("task.cancel({})").is_err());
    }

    #[test]
    fn an_unawaited_failure_that_settled_early_still_fails_the_run() {
        // The failure settles while the script is still running (it yields on
        // `task.wait`), so the drain finds nothing pending.
        let err = eval(
            r#"
            task.spawn(function() error("boom") end)
            task.wait(0.05)
        "#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("boom"), "{err}");
    }

    #[test]
    fn pool_cancel_wakes_a_parked_await() {
        // A concurrent await on a cancelled pool raises "drained" instead of
        // hanging forever.
        eval(
            r#"
            local pool = task.pool(1)
            pool:spawn(function() task.wait(10) return 1 end)
            local waiter = task.spawn(function()
                local ok, err = pcall(function() return pool:await() end)
                return ok, tostring(err)
            end)
            task.wait(0.05)
            assert(pool:cancel() == 1)
            local ok, err = task.await(waiter)
            assert(ok == false and err:find("drained"), err)
        "#,
        )
        .unwrap();
    }

    #[test]
    fn pool_cancel_claims_an_already_failed_task() {
        eval(
            r#"
            local pool = task.pool(2)
            pool:spawn(function() error("late") end)
            task.wait(0.05)
            pool:cancel()
        "#,
        )
        .unwrap();
    }

    #[test]
    fn durations_use_the_one_delta_grammar_and_never_panic() {
        for bad in ["1e20", "math.huge", "-1", "'-5s'", "'1month'", "'P1D'"] {
            assert!(
                eval(&format!("task.wait({bad})")).is_err(),
                "task.wait({bad})"
            );
        }
        eval("task.wait('10ms') task.wait(0)").unwrap();
    }

    #[test]
    fn pool_rejects_huge_and_fractional_concurrency() {
        for bad in ["1e19", "2.5", "'2'"] {
            assert!(eval(&format!("task.pool({bad})")).is_err(), "{bad}");
        }
        eval("task.pool(2.0)").unwrap();
    }

    #[test]
    fn status_reports_the_task_state() {
        eval(
            r#"
            local ok = task.spawn(function() return 1 end)
            local bad = task.spawn(function() error("x") end)
            local slow = task.spawn(function() task.wait(10) end)
            assert(task.status(slow) == "pending")
            pcall(task.await, bad)
            task.await(ok)
            assert(task.status(ok) == "done" and task.status(bad) == "failed")
            task.cancel(slow)
            assert(task.status(slow) == "cancelled")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn await_rejects_an_empty_list_and_a_hole() {
        assert!(eval("task.await({})").is_err());
        assert!(eval("task.await()").is_err());
        // A hole is a wrong-typed element, not the end of the list.
        assert!(eval("local h = task.spawn(function() end) task.await({h, nil, h})").is_err());
    }

    #[test]
    fn a_batch_error_names_the_failing_handle() {
        let err = eval(
            r#"
            local a = task.spawn(function() return 1 end)
            local b = task.spawn(function() error("nope") end)
            task.await(a, b)
        "#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("handle 2") && err.contains("nope"), "{err}");
    }

    #[test]
    fn nested_spawn_inside_a_pooled_task_cannot_await_a_pool() {
        eval(
            r#"
            local pool = task.pool(1)
            pool:spawn(function()
                local inner = task.spawn(function() return pcall(function() return pool:poll() end) end)
                return (task.await(inner))
            end)
            assert(pool:await() == false)
        "#,
        )
        .unwrap();
    }

    #[test]
    fn a_pool_has_a_readable_concurrency_and_tostring() {
        eval(
            r#"
            local p = task.pool(3)
            assert(p:concurrency() == 3)
            assert(tostring(p):find("task.pool%(3%)"))
        "#,
        )
        .unwrap();
    }
}
