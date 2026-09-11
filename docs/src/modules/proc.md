# `proc` — running commands

| Method | Effect |
| --- | --- |
| `proc.command(<program>)` / `proc.command(<list>)` | start a command builder |
| `:arg(<value>)` | append one argument |
| `:args(<list>)` | append several arguments |
| `:env(<name>, <value>)` | set an environment variable |
| `:envs(<table>)` | set several environment variables |
| `:env_remove(<name>)` | unset an environment variable in the child |
| `:env_clear()` | start the child from an empty environment |
| `:current_dir(<path>)` | working directory for the child |
| `:stdin(<source>)` | what the child reads (default `proc.NULL`) |
| `:stdout(<target>)` | where stdout goes (default `proc.NULL`) |
| `:stderr(<target>)` | where stderr goes (default `proc.NULL`) |

| Method | Effect |
| --- | --- |
| `:run()`, async | the result table, after the command exits |
| `:spawn()` | a `Child`, to be awaited later |
| `:exec()` | never returns on success — replaces this process |
| `:interactive()`, async | the result table; stdin/stdout/stderr default to this terminal |
| `Child:wait()`, async | the result table, after the child exits |
| `Child:try_wait()`, async | `nil` while running, else the result table |
| `Child:kill()`, async | the result table, after SIGKILL |
| `Child:signal(<signal>)` | nothing — the child stays yours to wait for |
| `Child:id()` | the OS process id |

| Function | Effect |
| --- | --- |
| `proc.pid()` | the running process's own pid (not in `ied`) |
| `proc.trap(<signal>, <handler>)` | react to a signal sent to *this* process |
| `proc.untrap(<signal>)` | stop reacting to a signal |
| `proc.exit([<code>])` | terminate this process immediately |

| Constant | Value |
| --- | --- |
| `proc.SIGHUP`, `SIGINT`, `SIGQUIT`, `SIGKILL`, `SIGUSR1`, `SIGUSR2`, `SIGPIPE`, `SIGALRM`, `SIGTERM`, `SIGCONT`, `SIGSTOP`, `SIGTSTP`, `SIGCHLD`, `SIGTTIN`, `SIGTTOU`, `SIGWINCH` | the platform's signal number |

Redirect-target constructors (`proc.NULL`, `proc.file()`, `proc.string()`,
...) live on the same table, installed by `redirect::install` — see
below for what `:stdout()`/`:stderr()` accept, and `redirect.md` for the
targets themselves.

## Example

```lua
local r = proc.command({ "git", "rev-parse", "HEAD" })
    :stdout(proc.string())
    :run()

if r.success then
    print("HEAD is", r.stdout)
end
```

## Conventions

**The builder pattern.** `proc.command(...)` returns a `CommandBuilder`;
every builder method mutates it in place and returns the same handle, so
calls chain. Nothing runs until a finalizer (`:run()`, `:spawn()`,
`:exec()` or `:interactive()`) is called. The four differ in where the
child's streams go by default:

| Finalizer | stdin | stdout / stderr | Redirects |
| --- | --- | --- | --- |
| `:run()`, `:spawn()` | `proc.NULL` | `proc.NULL` | all accepted |
| `:interactive()` | `proc.INHERIT` | `proc.INHERIT` | all accepted |
| `:exec()` | this process's | this process's | only `proc.INHERIT` (or unset) |

An explicit `proc.INHERIT` and leaving a stream unset therefore mean the
same thing to `:exec()`, while an explicit `proc.NULL` is rejected there:
there is no child to discard anything for.

Program, arguments, environment and directory are byte strings: they need
not be valid UTF-8 (a NUL byte is still rejected).

```lua
proc.command("ls"):arg("-la")
proc.command({ "ls", "-la" })
```

`command` accepts either a bare program name (arguments added via `:arg`/
`:args`) or a list holding the program and its arguments together —
`command({prog, args...})` is equivalent to `command(prog):args({args...})`,
and arguments from the list come before any added afterwards. A list whose
first element is missing (`command({})`) raises immediately.

Arguments are passed as an argv vector, never through a shell: nothing is
word-split or glob-expanded. To use shell features, invoke a shell
explicitly:

```lua
proc.command({ "sh", "-c", "ls *.txt | wc -l" })
```

**Redirect targets.** `:stdout()`/`:stderr()` take one of the handles
`proc` installs (documented fully in `redirect.md`): `proc.NULL` (discard,
the default for both streams), `proc.INHERIT` (pass through to the slot's
own counterpart on this program's side), `proc.OUT`/`proc.ERR` (pass
through to *this* program's stdout or stderr, named rather than matched by
slot), `proc.file(path)` (write to a local file, opened lazily on first
use), `proc.buffer()`/`proc.string()` (capture for retrieval in the result
table, as a `buffer` or a string respectively), and the aliases
`proc.STDOUT`/`proc.STDERR` (bind to the *other* stream's current target,
like the shell's `2>&1`/`1>&2` — resolved at the moment of assignment, not
when the command runs). `proc.STDOUT` is only valid on `:stderr()` and
`proc.STDERR` only on `:stdout()`; aliasing a stream onto itself raises.
Passing anything else (a plain string, say) raises with
`"expected a redirect target"`.

Because aliases bind at assignment, **order matters**, as in the shell:

```lua
-- 2>&1 after the capture: stderr joins stdout's capture
proc.command(cmd):stdout(proc.string()):stderr(proc.STDOUT):run()
-- 2>&1 before it: stderr aliases stdout's target *then* (NULL), and is lost
proc.command(cmd):stderr(proc.STDOUT):stdout(proc.string()):run()
```

An alias onto a stream that has not been configured binds to `proc.NULL`;
an alias onto a plain `proc.INHERIT` binds to the concrete stream it means
(`:stdout(proc.INHERIT):stderr(proc.STDOUT)` puts stderr on *this*
program's stdout). To merge two captures, sharing one handle is usually
clearer than an alias:

```lua
local both = proc.string()
proc.command(cmd):stdout(both):stderr(both):run()
```

**The result shape.** The result table always has `success`, plus `status`
or `signal`/`signal_name`. It has a `stdout`/`stderr` field **only** for a
stream whose target was `proc.buffer()`/`proc.string()`; with a merged
capture both fields hold the same value. Files, `proc.NULL` and the inherit
targets leave their field absent. (An alias onto a capture shares the
handle, so it yields both fields.)

**`proc.file(path, [<options>])`.** The path is resolved against this
process's working directory, *not* `:current_dir()`. The file is opened on
first use and the handle keeps the descriptor: reusing one handle for
several commands continues at the same offset (so later commands append)
until the handle is collected; create a new handle to truncate again.
`{ append = true }` opens with `O_APPEND` instead of truncating. The
"same file" check behind the separate-handles error compares device and
inode once both files exist, so a symlink or `a/../f` is caught too. All
targets are opened before the child is spawned, but a program that then
fails to launch has already truncated a file target.

**`:stdin`.** `proc.NULL` (the default), `proc.INHERIT`, `proc.file(path)`
(opened for reading, never created), or a Luau string or buffer, which is
written to the child's stdin from its own task and then closed — input
larger than a pipe buffer is fine. Capture handles and the other redirects
raise `"expected a stdin source"`.

`proc.OUT`/`proc.ERR` and `proc.STDOUT`/`proc.STDERR` are two different
questions. The first pair names one of *this* program's streams — a
child's stdout sent to ito's stderr, which `proc.INHERIT` cannot express
because it hands the child whichever of our descriptors matches the slot
it is assigned to. The second pair cross-links the *child's* two streams
to each other, wherever those happen to point.

Passing the *same* capture or file handle to both `:stdout()` and
`:stderr()` merges the two streams onto one descriptor — one write offset,
correctly interleaved output, and a merged capture reports the same value
under both `result.stdout` and `result.stderr`. Passing two *different*
`proc.file()` handles that happen to name the same path is rejected at
finalize time (`:run()`/`:spawn()`), since each would get its own write
offset and clobber the file.

**Async, not synchronous.** `proc` is registered with
`create_async_function`/`add_async_method`, and children run on
`tokio::process`: `:run()` and `Child:wait()`/`:kill()` are real yield
points, so several commands started under `task.spawn` genuinely overlap
rather than taking turns. Captured streams are drained on their own tasks
starting at spawn time (not lazily at `:wait()`), so a child that outruns
a pipe's buffer never deadlocks against a reader that hasn't started yet.
Without `task.spawn`, commands still run one after another — `proc` gives
you concurrency, it doesn't create it.

**Errors vs. results.** Failing to *start* the command (bad program name,
`ENOENT`, ...) raises a Luau error. A non-zero exit from a program that
did start is not an error: `result.success` is `false` and `result.status`
carries the code, so a script decides for itself what a failure means.
Calling `:wait()`/`:kill()` a second time on the same child raises, as
does `:id()` (or `:signal()`) once the child has already been waited for
or killed — both consume the child, so there is no way to leave a zombie
behind by forgetting to wait.

**How a child died.** A child killed by a signal has no exit code, so
`result.status` is `nil` — which said an exit code was missing without
saying why. The result therefore also carries `result.signal` (the
number) and `result.signal_name` (`"SIGTERM"`, ...), present **only** on a
signal death and both together, so `if result.signal then` is the test
for how a child died. A normal exit has both absent. `signal_name` is
absent for a number this module does not name, rather than being an
invented `"SIG37"`.

**Signal names and numbers.** Anywhere a signal is given (`:signal()`),
it may be a name without the prefix (`"TERM"`), with it (`"SIGTERM"`), in
any case (`"sigterm"`), a raw number (`15`), or one of the `proc.SIG*`
constants. The constants are plain numbers, taken from libc rather than
written out, because the job and user-defined signals genuinely differ
between Linux and macOS (`SIGUSR1` is 10 on one and 30 on the other) —
and being numbers, they also compare against `result.signal`:

```lua
if r.signal == proc.SIGTERM then ... end
```

A signal the module does not *name* is still sendable as a number: the
list bounds the names, not the capability.

**Two directions of signal.** Everything above (`Child:signal`, `Child:kill`)
sends a signal *to a child* this script spawned. `proc.trap`/`proc.untrap`/
`proc.exit` are the opposite direction: reacting to (or causing) a signal
sent *to this process*, the one running the script. There is no built-in
reaction to any signal — a signal nothing has trapped keeps its default
disposition, SIGINT and SIGTERM included, exactly as if `proc.trap` did not
exist.

## `proc.command`

Start building a command.

**Signature:** `proc.command(program: string) -> CommandBuilder` or
`proc.command(spec: {string}) -> CommandBuilder`

**Parameters:**
- `program` — the executable name or path, with no arguments.
- `spec` — a list whose first element is the program and whose remaining
  elements are its arguments, e.g. `{"ls", "-la"}`.

**Returns:** a `CommandBuilder` handle.

**Methods:** `arg` · `args` · `env` · `envs` · `env_remove` · `env_clear` ·
`current_dir` · `stdin` · `stdout` · `stderr` · `run` · `spawn` · `exec` ·
`interactive`

**Errors:** raises if `spec` is a table whose first element is missing
(an empty list) or whose elements aren't strings; raises if the value is
neither a string nor a table.

```lua
local a = proc.command("printf"):arg("%s-%s"):args({"a", "b"})
local b = proc.command({ "printf", "%s-%s", "a", "b" })
-- a and b build the same command
```

## `CommandBuilder:arg`

Append one argument to the command.

**Signature:** `builder:arg(value: string) -> CommandBuilder`

**Parameters:** `value` — the argument, passed literally (no shell
quoting or expansion applies).

**Returns:** the same builder, for chaining.

```lua
proc.command("git"):arg("commit"):arg("-m"):arg("message"):run()
```

## `CommandBuilder:args`

Append several arguments at once.

**Signature:** `builder:args(values: {string}) -> CommandBuilder`

**Parameters:** `values` — a list of arguments, appended in order after
any already present.

**Returns:** the same builder, for chaining.

```lua
proc.command("git"):args({ "commit", "-m", "message" }):run()
```

## `CommandBuilder:env`

Set one environment variable for the child.

**Signature:** `builder:env(name: string, value: string) -> CommandBuilder`

**Parameters:**
- `name` — the variable name.
- `value` — the variable's value.

**Returns:** the same builder, for chaining. Entries are applied in call
order; calling `:env` again with a name already set overwrites the
earlier value, last call wins.

```lua
proc.command("printenv"):arg("FOO"):env("FOO", "bar"):run()
```

## `CommandBuilder:envs`

Set several environment variables at once.

**Signature:** `builder:envs(vars: {[string]: string}) -> CommandBuilder`

**Parameters:** `vars` — a table of names to values; each pair is an
`:env()` call (pairs in no particular order).

**Returns:** the same builder, for chaining.

```lua
proc.command("make"):envs({ CC = "clang", JOBS = "8" }):run()
```

## `CommandBuilder:env_remove`

Unset one variable in the child, whether inherited or set earlier.

**Signature:** `builder:env_remove(name: string) -> CommandBuilder`

**Returns:** the same builder, for chaining. Applied in call order with
`:env()`.

```lua
proc.command("env"):env_remove("AWS_PROFILE"):run()
```

## `CommandBuilder:env_clear`

Start the child with an empty environment instead of inheriting ito's.

**Signature:** `builder:env_clear() -> CommandBuilder`

**Returns:** the same builder, for chaining. Like
`std::process::Command::env_clear`, it also forgets earlier `:env()`/
`:envs()`/`:env_remove()` calls; ones made afterwards apply on top of the
cleared environment.

```lua
proc.command("printenv"):env_clear():env("ONLY", "this"):run()
```

## `CommandBuilder:current_dir`

Set the child's working directory.

**Signature:** `builder:current_dir(path: string) -> CommandBuilder`

**Parameters:** `path` — the directory the child is spawned in. Relative
paths follow the same rules `std::process::Command::current_dir` does
(resolved against the parent's own current directory).

**Returns:** the same builder, for chaining.

```lua
proc.command({ "git", "status" }):current_dir("/some/repo"):run()
```

## `CommandBuilder:stdin`

Set what the child reads on standard input.

**Signature:** `builder:stdin(source: Redirect | string | buffer) -> CommandBuilder`

**Parameters:** `source` — `proc.NULL` (default for `:run()`/`:spawn()`),
`proc.INHERIT` (default for `:interactive()`), `proc.file(path)` to read a
local file, or a string or buffer whose bytes are fed to the child before
stdin is closed.

**Returns:** the same builder, for chaining.

**Errors:** raises with `"expected a stdin source"` for anything else
(capture handles, `proc.OUT`/`proc.ERR`, aliases). Raises at finalize time
if a `proc.file()` source cannot be opened.

```lua
local r = proc.command({ "wc", "-l" }):stdin("a\nb\n"):stdout(proc.string()):run()
```

## `CommandBuilder:stdout`

Set where the child's standard output goes.

**Signature:** `builder:stdout(target: Redirect) -> CommandBuilder`

**Parameters:** `target` — one of `proc.NULL`, `proc.INHERIT`,
`proc.OUT`, `proc.ERR`, `proc.file(path)`, `proc.buffer()`,
`proc.string()`, or `proc.STDERR` (alias to whatever stderr's target
currently is). Default (if never called) is `proc.NULL`
(`proc.INHERIT` under `:interactive()`).

**Returns:** the same builder, for chaining.

**Errors:** raises with `"expected a redirect target"` if `target` is not
one of the handles above (e.g. a plain string); raises if `target` is
`proc.STDOUT` (a stream aliased onto itself).

Assigning `proc.STDERR` binds *eagerly*, to stderr's target at the moment
of the call — like the shell's `1>&2`, a later `:stderr()` call does not
retroactively change what stdout already captured.

```lua
local r = proc.command("printf"):arg("hi"):stdout(proc.string()):run()
assert(r.stdout == "hi")
```

```lua
-- the shell's `1>&2`: the child's stdout onto *this* program's stderr
proc.command({ "sh", "-c", "echo x" }):stdout(proc.ERR):run()
```

## `CommandBuilder:stderr`

Set where the child's standard error goes. Same shape as `:stdout`,
mirrored: `proc.STDOUT` aliases to whatever stdout's target currently is.

**Signature:** `builder:stderr(target: Redirect) -> CommandBuilder`

**Parameters:** `target` — same set as `:stdout`'s, with the roles of
`proc.STDOUT`/`proc.STDERR` swapped. Default is `proc.NULL`.

**Returns:** the same builder, for chaining.

**Errors:** same as `:stdout`, with `proc.STDERR` the self-alias.

```lua
-- Merge stderr into stdout, shell-style `2>&1` (stdout first, see above):
local r = proc.command("sh"):arg("-c"):arg("echo o; echo e >&2")
    :stdout(proc.string())
    :stderr(proc.STDOUT)
    :run()
-- r.stdout contains both lines
```

```lua
-- `proc.OUT` is the other `2>&1`: onto *this* program's stdout
proc.command({ "sh", "-c", "echo x >&2" }):stderr(proc.OUT):run()
```

## `CommandBuilder:run`

Start the child and wait for it to exit, in one call.

**Signature:** `builder:run() -> table` (async)

**Returns:** the result table:

| Field | Type | Present |
| --- | --- | --- |
| `success` | boolean | always |
| `status` | number | unless the child was killed by a signal |
| `signal` | number | only on a signal death |
| `signal_name` | string | only on a signal death, and only for a signal `proc` names |
| `stdout` | string or buffer | only if stdout's target was `proc.buffer()`/`proc.string()` |
| `stderr` | string or buffer | only if stderr's target was `proc.buffer()`/`proc.string()` |

If stdout and stderr shared one capture handle, both fields hold the same
merged value. `signal`/`signal_name` appear together and only when the
child died by a signal, so `if r.signal then` tells the two kinds of
death apart where a bare `r.status == nil` only said an exit code was
missing.

**Errors:** raises if the program could not be launched (not found,
permission denied, ...) or if the same file was targeted by stdout and
stderr through two separate `proc.file()` handles. Does **not** raise on
a non-zero exit — that is reported via `success`/`status`.

The capture readers are joined before the exit is awaited, so a
grandchild that inherited the pipe and outlives the child keeps `:run()`
(and `:wait()`) waiting until it closes the pipe too. Redirect to a file or
`proc.NULL` if a daemonizing child should not be waited for.

If the run is abandoned (dropped mid-wait, e.g. by `task.timeout`), its
child is killed when the run is torn down.

```lua
local r = proc.command({ "ls" }):run()
if not r.success then
    log.warn("ls failed with status " .. tostring(r.status))
end
```

## `CommandBuilder:spawn`

Start the child without waiting for it.

**Signature:** `builder:spawn() -> Child`

**Returns:** a `Child` handle, to be finished later with `:wait()` or
`:kill()`.

**Errors:** raises under the same conditions as `:run()`'s launch
failures (a bad program name, a stdout/stderr file conflict); does not
wait for the child, so a later non-zero exit surfaces only from `:wait()`/
`:kill()`.

```lua
local child = proc.command({ "sleep", "5" }):spawn()
-- do other work here while it runs
local r = child:wait()
```

## `CommandBuilder:exec`

Replace this process with the child, via `execve`, exactly as the shell's
`exec` builtin does.

**Signature:** `builder:exec() -> ()`

**Returns:** nothing, on success — because there is no longer a process
here to return to. The calling process's image, pid and open file
descriptors all become the new program's; stdin/stdout/stderr are
inherited as-is, unchanged from whatever they already were.

The program is resolved the same way `:run()`/`:spawn()` resolve it: a
bare name is searched for on `$PATH` (the *child's*, so an `:env("PATH", ...)`
applies), a name containing `/` is used as-is. Output this process has
printed is flushed first.

**Errors:** raises if `:stdin()`/`:stdout()`/`:stderr()` were set to
anything but `proc.INHERIT`: there is no fork to redirect within, so a
capture, file or NULL target has no process left to apply to. Raises if the
program could not be launched (not found, permission denied, ...) — in that
case nothing has been replaced, the working directory is back where it was,
and the error surfaces normally. A permission failure on one `$PATH` entry is
reported in preference to "not found" from the rest.

```lua
-- Hand off to another program entirely, e.g. a wrapper script re-execing
-- the real binary with adjusted arguments.
proc.command({ "the-real-program", "--flag" }):exec()
-- unreached on success
```

## `CommandBuilder:interactive`

Start the child on this process's terminal and wait for it to exit:
stdin, stdout and stderr default to `proc.INHERIT`.

**Signature:** `builder:interactive() -> table` (async)

**Returns:** the same result table as `:run()`. `stdout`/`stderr` fields
appear only for a stream explicitly redirected to a capture.

**Errors:** raises if the program could not be launched or a target cannot
be opened. Does **not** raise on a non-zero exit.

While the child runs, this process ignores SIGINT and SIGQUIT (the child
gets the default disposition back), so ^C reaches the child — an editor
deciding what it means — rather than killing the script. The previous
disposition, a `proc.trap` handler included, is restored afterwards.

Neither `:run()`/`:spawn()` (NULL by default) nor `:exec()` (never returns
to Luau) gives a child the real terminal; `:interactive()` is for a child
that needs one — a pager, an editor, anything prompting for input. Any
stream can still be redirected explicitly, which is the `fzf`/`gum` case:
the UI stays on the terminal while the selection is captured.

```lua
local r = proc.command("vim"):arg("notes.txt"):interactive()
if not r.success then
    log.warn("vim exited with status " .. tostring(r.status))
end

local pick = proc.command({ "fzf" }):stdin(list_text):stdout(proc.string()):interactive()
print("picked", pick.stdout)
```

## `Child:wait`

Wait for a spawned child to exit and collect its result.

**Signature:** `child:wait() -> table` (async)

**Returns:** the same result table shape as `:run()`.

**Errors:** raises if this child has already been waited for or killed
(`"this child has already been waited for"`) — both `:wait()` and
`:kill()` consume the child, so calling either a second time is an error
rather than returning a stale result.

```lua
local child = proc.command({ "sleep", "1" }):spawn()
local r = child:wait()
assert(r.success)
```

## `Child:try_wait`

Check whether a spawned child has exited, without waiting.

**Signature:** `child:try_wait() -> table | nil` (async)

**Returns:** `nil` while the child is still running; once it has exited,
the same result table `:wait()` returns (and the child is consumed, like
`:wait()`). The grace-period pattern can stop as soon as the child is gone:

```lua
child:signal("TERM")
local r
for _ = 1, 50 do
    r = child:try_wait()
    if r then break end
    task.wait("100ms")
end
r = r or child:kill()
```

**Errors:** raises if the child has already been waited for or killed.

## `Child:kill`

Send SIGKILL to a spawned child, then reap it.

**Signature:** `child:kill() -> table` (async)

**Returns:** the result table `:wait()` would have returned, including
any output the child managed to write before dying. `status` is absent
(signal deaths have no exit code), `signal` is `proc.SIGKILL` and
`signal_name` is `"SIGKILL"`, and `success` is `false`.

**Errors:** raises if the child has already been waited for or killed, or
if the kill signal itself could not be sent.

`:kill()` signals and then reaps; it does not check whether the child was
still alive. If the child had already exited on its own (but was not yet
waited for), the result is its real exit status, not a `SIGKILL` death — test
`r.signal` rather than assuming.

Killing reaps the child itself, so `:kill()` followed by `:wait()` on the
same handle raises just like calling `:wait()` twice — there is no way to
leave a zombie behind by forgetting to wait. A `:spawn()`ed child that is
abandoned (its handle dropped without `:kill()`/`:wait()`) keeps running and
is reaped by tokio's SIGCHLD handling once it ends: `:kill()` is what
actually stops it.

```lua
local child = proc.command({ "sleep", "30" }):spawn()
local r = child:kill()
assert(r.success == false)
assert(r.status == nil)  -- died by signal, no exit code
assert(r.signal_name == "SIGKILL")
```

## `Child:signal`

Send an arbitrary signal to a spawned child, without consuming it.

**Signature:** `child:signal(sig: string | number) -> ()`

**Parameters:** `sig` — a signal name without the prefix (`"TERM"`), with
it (`"SIGTERM"`), in any case (`"sigterm"`), a raw number (`15`), or one
of the `proc.SIG*` constants. A signal `proc` does not name is still
sendable as a number.

**Returns:** nothing. Unlike `:kill()`, this does **not** consume the
child: a signal is a request, not a verdict — SIGTERM may be ignored,
delayed or handled, and SIGHUP/SIGUSR1 need not end the process at all.
Consuming the child here would force the caller to treat "asked to stop"
as "stopped". So `:wait()` remains yours to do, and the child stays live
long enough for a second signal — which is what makes the TERM-then-KILL
escalation expressible.

**Errors:** raises with `"unknown signal '<x>'; expected one of ..., or a
number"` for a name it does not recognise (a typo must not silently send
nothing, which would look like a process ignoring the signal); raises for
a number that is not an integer or does not fit a 32-bit integer, and for
any other type of value; raises
with the `"already been waited for"` error if the child has already been
waited for or killed; raises if `kill(2)` itself fails.

```lua
local child = proc.command({ "some-server" }):spawn()

child:signal("TERM")            -- or proc.SIGTERM, or 15
task.wait("10s")                -- a grace period, not a timed :wait()
child:signal(proc.SIGKILL)      -- still ours to signal again
local r = child:wait()
print(r.signal_name)            --> "SIGTERM" or "SIGKILL"
```

Note the plain `task.wait` rather than a `task.timeout` around
`child:wait()`: a timed-out `:wait()` leaves the child unusable for the
rest of the run — see [Bounding how long a command may
take](#bounding-how-long-a-command-may-take). Signalling an
already-exited child is harmless, since the child is not reaped until
`:wait()` and so the pid is still the one that was spawned.

## `Child:id`

Read the child's OS process id.

**Signature:** `child:id() -> number`

**Returns:** the pid, as a number.

**Errors:** raises once the child has already been waited for or killed
— read the pid before finishing the child if you need it afterwards (e.g.
for a watchdog).

```lua
local child = proc.command({ "long-job" }):spawn()
local pid = child:id()
```

## `proc.pid`

The running process's own pid (`std::process::id()`).

**Signature:** `proc.pid() -> number`

**Returns:** the pid, as an integer.

**Errors:** none.

Not available in `ied` — a pid is ambient state that
differs between two runs of one script, exactly what a reproducible run
must not read. It is absent rather than stubbed, so `if proc.pid then` is
a usable feature test.

It is a *pid*, not a lock — pair it with `proc.command` or a file under
`/tmp` if you need one, and remember a `deploy.<pid>.lock` file only names
the holder; it does not stop a second run.

```lua
log.info("running as pid " .. proc.pid())
local tmp = path.join("/tmp", "deploy." .. proc.pid() .. ".json")
```

## `proc.trap`

React to a signal sent to this process.

**Signature:** `proc.trap(signal: string | number, handler: (signal: number) -> ()) -> ()`

**Parameters:**
- `signal` — same set of spellings as `Child:signal`'s: a name without the
  prefix (`"TERM"`), with it (`"SIGTERM"`), in any case, a raw number
  (`15`), or one of the `proc.SIG*` constants.
- `handler` — called with the signal number on every delivery from this
  point on.

**Returns:** nothing.

**Errors:** raises for `proc.SIGKILL`/`proc.SIGSTOP` (or their numbers) —
the kernel never offers either to a handler; raises with `"cannot trap
signal <n>"` for `SIGILL`/`SIGFPE`/`SIGSEGV`/`SIGBUS` and for numbers that
are no signal, because the OS registration is attempted synchronously, at
the call; raises for an unrecognised signal name, matching `Child:signal`.

The **first** `proc.trap` call for a given signal is what takes it off its
default disposition; a signal never trapped behaves exactly as if this
function did not exist. Calling `proc.trap` again for the **same** signal
replaces the handler in place rather than adding a second one — there is
never more than one handler per signal.

```lua
proc.trap(proc.SIGTERM, function(sig)
    log.info("caught", sig, "-- shutting down")
    proc.exit(0)
end)
```

A signal that arrived **before** the first `proc.trap` call naming it is
not caught by anything this installs — trap early if a signal early in the
run matters.

The handler runs detached from the script's own control flow: there is no
`task` handle for it, so an error the handler raises is logged and the trap
keeps working for the next delivery, rather than failing the run the way an
un-awaited `task.spawn` does. A handler that means to end the process calls
[`proc.exit`](#procexit) itself.

**Delivery count.** Luau does not see a count directly from `proc.trap`, but
nothing stops a script from keeping its own inside the handler:

```lua
local hits = 0
proc.trap(proc.SIGUSR1, function()
    hits += 1
end)
```

POSIX signal delivery does not queue: if the same signal arrives twice
before the handler for the first delivery has run, the two deliveries
collapse into one call. A count kept this way is therefore a lower bound on
how many times the signal actually arrived, not an exact count — a signal
that must never be missed needs a mechanism other than a repeated signal
(a file, a socket, a second signal number) to make each occurrence
distinguishable in the first place.

## `proc.untrap`

Stop reacting to a signal.

**Signature:** `proc.untrap(signal: string | number) -> ()`

**Parameters:** `signal` — same spellings as `proc.trap`'s.

**Returns:** nothing, whether or not `signal` had a handler — untrapping a
signal nothing ever trapped is a no-op, not an error, so a script can call
this defensively without checking first.

```lua
proc.trap(proc.SIGHUP, function() reload_config() end)
-- ... later, once reloading no longer makes sense ...
proc.untrap(proc.SIGHUP)
```

After `proc.untrap`, the signal's process-level disposition is "caught,
does nothing" rather than reverting to the kernel default — it was already
taken off the default by the first `proc.trap` call, and nothing puts it
back. A script that needs the literal default behaviour (e.g. terminate)
back should have its own handler do that, or call [`proc.exit`](#procexit).

## `proc.exit`

Terminate this process immediately.

**Signature:** `proc.exit(code: number?) -> ()` (never returns)

**Parameters:** `code` — the process exit code. Defaults to `0`.

**Returns:** never — this is a direct `std::process::exit`, not a Luau-level
unwind. No `Drop`, no pending `task`, no buffered-write flush, nothing queued in
`proc`/`ssh` runs between the call and the process ending. That is
deliberate: a `proc.trap` handler that has decided the process must end
right now should not itself be at the mercy of something else being stuck
(a hung `child:wait()`, a wedged `task.pool`). A script that wants cleanup
to run first must do it *before* calling `proc.exit`, not rely on it running
afterward.

```lua
proc.trap(proc.SIGTERM, function()
    log.info("shutting down")
    proc.exit(0)
end)
```

## Running commands concurrently

`:run()` and `:wait()`/`:kill()` yield rather than blocking, so several
commands under [`task`](task.md) overlap:

```lua
local handles = {}
for _, dir in ipairs(repos) do
    table.insert(handles, task.spawn(function()
        return proc.command({ "git", "-C", dir, "fetch" }):run()
    end))
end
task.await(handles)
```

Without `task.spawn`, commands run one after another.

## Bounding how long a command may take

`task.timeout` puts a deadline on a command, since `:run()` yields:

```lua
local r = task.timeout("30s", function()
    return proc.command({ "git", "-C", dir, "fetch" }):run()
end)
if r == task.TIMEOUT then
    log.warn("fetch did not finish in 30s")
end
```

`task.timeout` bounds how long the script **waits**. A timed-out `:run()`
owns its child, so the child is killed when the abandoned run is dropped. A
`:spawn()`ed child is **not** killed by an abandoned wait: it keeps running
until you signal it, and tokio's SIGCHLD handling collects it once it ends.

Worse, a timed-out `:wait()` **leaves the handle unusable for the rest of
the run**: the abandoned wait took the child out, and `task.timeout`
cancels by aborting the task that holds it — which tokio drops only at
teardown, never mid-run. So `:kill()`, `:signal()` and `:id()` all report
`"already been waited for"` afterwards, and the child cannot be stopped
through the handle at all. (A `:wait()` future dropped *promptly* does
give the child back; `task.timeout` is the case that does not.)

So don't wrap `child:wait()` in a `task.timeout`. Either sleep for the
grace period and signal the handle you still hold:

```lua
local child = proc.command({ "long-job" }):spawn()
child:signal("TERM")
task.wait("30s")
child:signal("KILL")
local r = child:wait()
```

or, to bound the process while another task does the waiting, race it
against a watchdog that signals the handle (not a pid: a pid read earlier
can be reused by an unrelated process once the child is gone):

```lua
local child = proc.command({ "long-job" }):spawn()

local watchdog = task.spawn(function()
    task.wait("30s")
    child:signal("KILL")
end)

local r = child:wait()      -- returns normally once the watchdog fires
task.cancel(watchdog)       -- finished in time; stand the watchdog down

if r.signal then
    log.warn("long-job was killed by " .. (r.signal_name or r.signal))
end
```
