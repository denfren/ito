# `task` — concurrent tasks

| Function | Effect |
| --- | --- |
| `task.spawn(f, ...)` | start `f(...)` on its own task now; returns a handle |
| `task.await(...)` | wait for one or more handles; return their results |
| `task.poll(<handle>)` | check a handle without blocking |
| `task.cancel(<handle>)` | stop a pending task |
| `task.wait(<duration>)` | non-blocking sleep |
| `task.timeout(<duration>, f, ...)` | run `f(...)` under a deadline |
| `task.TIMEOUT` | sentinel `task.timeout` returns on expiry |
| `task.pool(<concurrency>)` | create a bounded pool |

| Pool method | Effect |
| --- | --- |
| `pool:spawn(f, ...)` | queue `f(...)`; returns nothing |
| `pool:await()` | block for the next finished task; return its values |
| `pool:poll()` | check the pool without blocking |
| `pool:count()` | tasks spawned but not yet collected |
| `pool:done()` | `true` once `count()` is `0` |
| `pool:cancel()` | stop every uncollected task; return how many it stopped |

## Example

```lua
local pool = task.pool(8)

for _, host in ipairs(hosts) do
    pool:spawn(function()
        return host, ssh.connect(host):command({ "uptime" }):stdout(proc.string()):run()
    end)
end

while not pool:done() do
    local host, result = pool:await()
    print(host, result.stdout)
end
```

Eight connections are open at any moment; the ninth starts as soon as one
finishes. Results arrive in the order they complete, not the order they
were spawned, so one slow host never holds up the others.

## General rules

Tasks share one thread and interleave only where they *await* — at a
`task.wait`, or inside a `proc`/`ssh`/`aws.ec2` call. Pure computation
never yields, so this speeds up waiting, not work, and tasks need no
locking around shared state (nothing runs simultaneously).

**`task.spawn` vs `task.pool`.** `task.spawn` starts work immediately, so
a loop over a thousand hosts opens a thousand connections at once.
`task.pool(n)` bounds concurrency to `n` and is not a gated `task.spawn`:
`pool:spawn` returns **nothing** — the pool owns the result, and
`pool:await`/`pool:poll` are the only way to read it. That leaves one
collection path instead of two that would have to agree about who
claimed a failure; the cost is that a result carries no handle to
identify it, so a task returns its own identity (as the example above
returns `host` alongside the probe result) — better than an opaque
`task#7`, since the script picks the name. A pooled task's permit is
acquired *inside* the pool's driver, before the task body's first resume,
so `pool:spawn` itself never blocks and a queued task has run no script
code yet.

**Completion order, not spawn order.** `pool:await`/`pool:poll` return
results as tasks *finish*, which is the whole point over
`task.await(<list>)`: a slow task does not hold up the ones behind it.
Nothing about the return order lines up with the spawn loop.

**Drained-pool errors raise, never `nil`.** `pool:await`/`pool:poll`
raise when nothing is left to collect, rather than returning `nil` —
`nil` would be indistinguishable from a task that legitimately returned
`nil` first. `pool:done()` (`count() == 0`) and `pool:count()` are one
fact read two ways; guard a collection loop with either. The same
ambiguity is why `task.timeout` has a dedicated `task.TIMEOUT` sentinel
instead of returning `nil` on expiry: a timed function's own return
value must stay distinguishable from "too slow."

**Reentrancy.** A pooled task holds its permit while parked, so awaiting
or polling *any* pool from inside pooled work raises rather than hanging
— done from within, once `concurrency` tasks did it, the pool would
deadlock with no error and no deadline to break it. The check is scoped
to the task body actually running: a pooled task parked on `task.wait` is
not "inside pooled work" for this purpose, only a task body currently
being resumed is, so the main script's own top-level `pool:await` is
never mistaken for reentrant pooled work. Plain `task.spawn`/`task.await`
nested inside pooled work is unaffected and is the documented escape for
work that needs to compose.

**Failure and the drain.** Every task — spawned directly, pooled, or
started by `task.timeout` — is registered and *drained* after the script
(and `main`, if defined) returns, before buffered `fs` writes are
flushed, so a task kept purely for its side effects still lands its
writes even if nobody awaited it. A task that fails without ever being
collected is logged and fails the run; a failure the script collected
with `pcall` (via `task.await`, `task.poll`, `pool:await` or
`pool:poll`) does not. `pool:cancel()` (and `task.cancel` for a plain
handle) is the only way to abandon work without failing the run —
cancellation is exempt from drain reporting.

**Durations.** Anywhere a duration is taken (`task.wait`, `task.timeout`)
it uses the same grammar as `time.delta` — a number of seconds
(fractional allowed) or a humantime string (`"30s"`, `"5min"`,
`"1h 30min"`). See [`time`](time.md) for the full grammar; there is no
separate parser here.

**`require` from a task is safe.** Luau resolves an import synchronously
within one resume, so no other task can run between the requirer
resetting and the `loader` call that ends the sequence — a concurrent
`require` cannot interleave mid-navigation.

## `task.spawn(f, ...)`

Starts `f(...)` on its own task and returns a handle immediately.

**Signature:** `task.spawn(f: function, ...: any) -> handle`

**Parameters**

- `f` — the function to run. Required; a non-function raises.
- `...` — extra arguments, passed to `f` unchanged.

**Returns:** a task handle (opaque userdata; `tostring(handle)` reads
`"task#<id> (<state>)"` with state one of `pending`/`done`/`failed`/
`cancelled`).

**Errors:** raises immediately if the first argument is missing or not a
function. Otherwise `spawn` itself cannot fail — an error raised *inside*
`f` is not raised here; it surfaces later, at `task.await`/`task.poll` (or,
if nobody collects it, at drain time), the same way `coroutine.resume`
reports a failure rather than raising it at the resume site. The task
makes no progress until the spawner itself next reaches an await point,
not at the `spawn` call.

```lua
local h = task.spawn(function(x, y) return x + y end, 2, 3)
print(task.await(h))  --> 5
```

## `task.await(...)`

Waits for one or more tasks and returns their results.

**Signature:** `task.await(handle, ...) -> ...` or `task.await(list) -> ...`

**Parameters**

- Either one or more task handles as separate arguments (`task.await(a, b)`),
  or a single table of handles (`task.await(handles)`) — the form a loop
  that accumulates handles into a table wants.

**Returns**

- With **one** handle: that task's return values, unchanged (so a
  multi-value return survives intact and a single `await` reads like a
  plain call).
- With **several** handles: one value per handle, positionally — a
  multi-value task result is *not* flattened, since that would make the
  positions stop lining up with the handles.

**Errors:** if any awaited task failed, `await` still waits for *every*
handle to settle first, then raises the first failure in list order.
Waiting for all of them (rather than returning at the first failure)
is what makes wrapping the call in `pcall` catch the whole batch: an
un-awaited failure is what the drain fails the run with, and bailing
early would leave later handles in the list unawaited and so still able
to fail the run from outside the `pcall`. Awaiting an already-finished
task returns its stored result (awaiting twice is fine); several tasks
may await the same handle.

```lua
local hs = {}
for i = 1, 4 do
    hs[i] = task.spawn(function() task.wait(0.5) return i end)
end
local a, b, c, d = task.await(hs)
```

## `task.poll(<handle>)`

Checks a task without blocking.

**Signature:** `task.poll(handle) -> ready: boolean, ...`

**Parameters**

- `handle` — a task handle. A non-handle value raises.

**Returns:** `false` alone while the task is still running; `true`
followed by the task's return values once it has finished. Polling does
not consume the result — a later `task.poll` or `task.await` on the same
handle still sees it.

**Errors:** if the task failed, `poll` raises the task's error — same
convention as `task.await`.

```lua
local h = task.spawn(function() task.wait(0.2) return "v" end)
assert(task.poll(h) == false)
task.wait(0.4)
local ready, value = task.poll(h)  -- ready == true, value == "v"
```

## `task.cancel(<handle>)`

Stops a task that has not finished yet.

**Signature:** `task.cancel(handle) -> boolean`

**Parameters**

- `handle` — a task handle. A non-handle value raises.

**Returns:** `true` if the task was pending and is now cancelled;
`false` if it had already finished or was already cancelled — a
finished task keeps its result, since cancelling cannot undo work
already done.

**Errors:** none beyond the non-handle case above; cancelling twice is
not an error, it just reports `false` the second time.

A cancelled task is neither driven to completion nor reported by the
drain — the run does not wait for it, and awaiting it raises a
"cancelled" error. Cancelling only reaches the task's own Luau body: a
local `proc` child it started is abandoned rather than killed, and a
remote `ssh` command keeps running on its host (SSH has no signal
channel to stop it with).

```lua
local h = task.spawn(function() task.wait(30) return "never" end)
assert(task.cancel(h) == true)
assert(task.cancel(h) == false)  -- already settled
```

## `task.wait(<duration>)`

Sleeps without blocking the runtime, so other tasks progress meanwhile.

**Signature:** `task.wait(duration) -> ()`

**Parameters**

- `duration` — seconds as a number (fractional allowed, non-negative),
  or a humantime string (`"30s"`, `"5min"`, `"1h 30min"`). See
  [`time`](time.md) for the shared delta grammar.

**Returns:** nothing.

**Errors:** raises if `duration` is negative, non-finite, an unparseable
string, or a value of the wrong type.

```lua
task.wait(0.5)
task.wait("500ms")
```

## `task.timeout(<duration>, f, ...)`

Runs `f(...)` under a deadline.

**Signature:** `task.timeout(duration, f: function, ...: any) -> ...`

**Parameters**

- `duration` — same grammar as `task.wait`.
- `f` — the function to run under the deadline.
- `...` — extra arguments, passed to `f` unchanged.

**Returns:** whatever `f` returned, all values unchanged, if it finished
before the deadline. If the deadline passed first: the single value
`task.TIMEOUT`.

**Errors:** an error raised by `f` **propagates** here — catch it with
`pcall`, same as `task.await`. Only the deadline produces the sentinel,
which is what makes `result == task.TIMEOUT` mean "too slow" and nothing
else. A missing/non-function `f`, a missing duration, or an unparseable
duration all raise before `f` ever runs.

`f` runs as an ordinary task underneath (registered, drained, cancellable),
so several `task.timeout` calls run concurrently with each other and with
plain `task.spawn` work. On expiry the task is cancelled — the same reach
as `task.cancel`: a local `proc` child is abandoned rather than killed, a
remote `ssh` command keeps running on its host. A timed-out task is
cancelled, so it is not reported by the drain even if its body would have
raised.

```lua
local address = task.timeout("20min", function()
    return get_leader_address(cluster)
end)
if address == task.TIMEOUT then
    log.warn("leader lookup timed out")
    return
end
```

## `task.TIMEOUT`

The sentinel `task.timeout` returns when its deadline passes first.

**What is it:** a unique userdata value (`tostring(task.TIMEOUT) ==
"task.TIMEOUT"`). Identity is the point: a script cannot construct
another value `==` to it by accident, so `result == task.TIMEOUT` is a
reliable test even when the timed function's own return values are
arbitrary strings, tables, or `nil`.

```lua
local r = task.timeout("50ms", function() task.wait(30) return "never" end)
assert(r == task.TIMEOUT)
```

## `task.pool(<concurrency>)`

Creates a pool that runs at most `concurrency` task bodies at once.

**Signature:** `task.pool(concurrency: number) -> pool`

**Parameters**

- `concurrency` — an integer of at least `1`. `0`, negative, or
  non-integer values raise.

**Returns:** a pool object (see the `pool:*` methods below).

```lua
local pool = task.pool(8)
```

## `pool:spawn(f, ...)`

Queues `f(...)` on the pool.

**Signature:** `pool:spawn(f: function, ...: any) -> ()`

**Parameters**

- `f` — the function to run. Required; a non-function raises.
- `...` — extra arguments, passed to `f` unchanged.

**Returns:** nothing — deliberately. The pool owns the result; `pool:await`
and `pool:poll` are the only way to it.

**Errors:** raises if `f` is missing or not a function.

The task's permit is acquired inside the pool's own driver, before the
body's first resume: `pool:spawn` never blocks, and a task queued behind
a full pool has executed no script code yet.

```lua
local pool = task.pool(2)
pool:spawn(function() return "host-a", probe("host-a") end)
```

## `pool:await()`

Blocks until the pool has a result ready, and returns it.

**Signature:** `pool:await() -> ...`

**Returns:** the next finished task's return values, unchanged (so a
task's own multi-value return survives, same as a single-handle
`task.await`). Results come back in **completion order**, not spawn
order.

**Errors:**

- Re-raises the task's error if that task's body failed — catch with
  `pcall`.
- Raises "on a drained pool" if `pool:count()` is already `0` (nothing
  left to wait for) — never returns `nil` for this case, since a task
  may legitimately have returned `nil` itself.
- Raises a reentrancy error if called from inside any pool's own task
  body (see General rules).

```lua
local pool = task.pool(4)
for _, d in ipairs({ 0.3, 0.05, 0.2, 0.1 }) do
    pool:spawn(function() task.wait(d) return d end)
end
local got = {}
while not pool:done() do
    table.insert(got, pool:await())
end
-- got == { 0.05, 0.1, 0.2, 0.3 }: completion order, not spawn order.
```

## `pool:poll()`

Checks the pool without blocking.

**Signature:** `pool:poll() -> ready: boolean, ...`

**Returns:** `false` alone if nothing has finished yet; `true` followed
by the next finished task's values (completion order, as with
`pool:await`).

**Errors:**

- Re-raises a collected task's error, same as `pool:await`.
- Raises "on a drained pool" if `pool:count()` is `0` — same wording and
  reasoning as `pool:await`.
- Raises the same reentrancy error as `pool:await` if called from inside
  any pool's own task body.

```lua
local pool = task.pool(2)
pool:spawn(function() task.wait(0.2) return "v" end)
assert(pool:poll() == false)
task.wait(0.4)
local ready, value = pool:poll()  -- true, "v"
```

## `pool:count()`

**What is it:** how many spawned tasks have not yet been collected —
queued, running, and finished-but-uncollected all count.

**Signature:** `pool:count() -> number`

**Returns:** the count. `pool:done()` is exactly `pool:count() == 0` —
they are one fact read two ways, so either can guard a collection loop.

```lua
local pool = task.pool(2)
for i = 1, 3 do pool:spawn(function() return i end) end
assert(pool:count() == 3)
pool:await()
assert(pool:count() == 2)  -- collecting decrements it
```

## `pool:done()`

**What is it:** whether every spawned task has been collected.

**Signature:** `pool:done() -> boolean`

**Returns:** `true` once `pool:count() == 0` (including for a pool
nothing was ever spawned on); `false` otherwise.

```lua
local pool = task.pool(2)
assert(pool:done() == true)  -- empty pool is done
pool:spawn(function() return 1 end)
assert(pool:done() == false)
pool:await()
assert(pool:done() == true)
```

## `pool:cancel()`

Stops every task the pool has not yet collected — queued, running, and
finished-but-uncollected.

**Signature:** `pool:cancel() -> number`

**Returns:** how many tasks it actually stopped (a finished, uncollected
task result is discarded, not stopped in the same sense as a still-running
one, but is also cleared from the pool).

**Errors:** none.

Cancellation is exempt from drain reporting, so `pool:cancel()` is the
only way to abandon a sweep of pooled work without failing the run —
including tasks that would otherwise have failed. After it, `pool:done()`
is `true` and `pool:count()` is `0`.

```lua
local pool = task.pool(2)
for i = 1, 6 do
    pool:spawn(function() task.wait(30) error("never seen") end)
end
local n = pool:cancel()
assert(n > 0)
assert(pool:done() == true)  -- the run still succeeds
```
