# `ssh` — running commands remotely

Not available in `ied`: remote commands are network and process state an `ied`
run must not depend on ([reproducibility](../binaries/ied.md#reproducibility)).

| Function | Effect |
| --- | --- |
| `ssh.connect(<host>, [<options>])` | a connection (yields) |

| Method | Effect |
| --- | --- |
| `:host()` | the host it was opened for |
| `:check()` | `true`, or `false` and the reason (yields) |
| `:close()` | shut the session down (yields) |
| `:command(<program>)` / `:command(<list>)` | a remote command builder |

| Method | Effect |
| --- | --- |
| `:arg(<value>)` | append one argument |
| `:args(<list>)` | append several arguments |
| `:stdin(<source>)` | what the command reads (default `proc.NULL`) |
| `:stdout(<target>)` | where stdout goes (default `proc.NULL`) |
| `:stderr(<target>)` | where stderr goes (default `proc.NULL`) |
| `:run()` | the result, after the command exits (yields) |
| `:spawn()` | a remote child, awaited with `:wait()` (yields) |
| `:interactive()` | run on a remote pty attached to this terminal (yields) |
| `:env`, `:envs`, `:env_remove`, `:env_clear`, `:current_dir`, `:exec` | always raise: no equivalent over SSH |

| Method | Effect |
| --- | --- |
| `:wait()` | the result, after the child exits (yields) |
| `:try_wait()` | `nil` while running, else the result (yields) |
| `:kill`, `:signal`, `:id` | always raise: SSH has no signal channel or local pid |

Runs commands on another machine, mirroring [`proc`](proc.md). A
hostname is all that is needed — `User`, `HostName`, `ProxyJump`,
`IdentityFile` and the rest come from the machine's ssh config, because
this drives the real `ssh` binary rather than reimplementing the
protocol.

```lua
local s = ssh.connect("prod")
local r = s:command({ "systemctl", "is-active", "nginx" })
    :stdout(proc.string())
    :run()
print(r.stdout)
```

## Conventions

- **Builder and terminal.** `ssh.connect(host)` returns a `Connection`.
  `conn:command(...)` returns a `RemoteCommand` builder; every configuration
  method (`:arg()`, `:args()`, `:stdin()`, `:stdout()`, `:stderr()`) mutates
  that builder in place and returns the same handle, so calls chain. The
  terminals are `:run()` and `:interactive()` (both async, both return the
  result table) and `:spawn()` (returns a `RemoteChild`, collected with
  `:wait()`/`:try_wait()`). `Connection:command()` validates its program
  immediately; argument types raise at the setter, and connection failures
  surface at `connect` or at the terminal. The no-equivalent methods
  (`:env()`, `:kill()`, …) are present only to raise.
- **Authentication is entirely the ssh config's job.** There is no
  password/key/agent option on `connect` — whatever `~/.ssh/config`
  (or system config) says for that host name is what runs, exactly as
  typing `ssh <host>` would.
- **Host keys are checked strictly.** An unknown or *changed* key is an
  error, and ssh is never allowed to prompt (the underlying crate always
  passes `BatchMode=yes`). Add the host to `known_hosts` first (e.g. an
  interactive `ssh <host>` once), or opt into trust-on-first-use with
  `ssh.connect(host, { tofu = true })`: an unknown host is then recorded,
  while a *changed* key is still an error. Either way the policy is passed as
  an `-o` override, so it wins over `StrictHostKeyChecking` in the ssh
  config. A host whose key legitimately rotates needs
  `UserKnownHostsFile /dev/null` in its config (with `tofu = true`) so every
  connect looks like a first connect.
- **The destination** is anything `ssh` accepts: a config alias,
  `user@host`, or an `ssh://user@host:port` URI.
- **The program and arguments are shell-escaped, not shell-free.** Each word
  reaches the remote side as exactly the string given, so nothing in them
  can inject a command. But sshd always runs the command line through the
  user's login shell, so there is a remote shell involved: for redirections,
  variables, pipes or `cd`, say so explicitly with `{ "sh", "-c", "..." }`.
  Programs and arguments must be valid UTF-8 (unlike `proc`).
- **One connection, one control master.** Each `ssh.connect()` is an
  independent session with its own control master, so several
  connections — to the same host or different ones — coexist, and every
  command on one session reuses its single handshake rather than paying
  for a new one.
- **Redirect targets are shared with `proc`.** `:stdout()`/`:stderr()`
  take the exact same values as `proc`'s — `proc.NULL`, `proc.INHERIT`,
  `proc.OUT`/`proc.ERR`, `proc.STDOUT`/`proc.STDERR`, `proc.file(path)`,
  `proc.buffer()`, `proc.string()` — and `:stdin()` the same sources (NULL,
  INHERIT, `proc.file(path)`, a string or buffer); see [`proc`](proc.md) for
  what each means and how merging works. A merged capture (one handle on both
  streams) shares one local pipe, so the two interleave in arrival order, as
  they do locally. Output is drained from `:spawn()` onward, so a spawned
  command that writes a lot does not stall before `:wait()`. `proc.file(path)` always names a file on **this**
  machine, even for a remote command: the target is a local file
  descriptor, so a remote command's output travels back over the
  connection to be written here. To write on the far side, redirect
  inside the remote command itself:

  ```lua
  s:command({ "sh", "-c", "dump > /remote/backup.sql" }):run()
  ```
- **Everything that talks to the network yields, not blocks.**
  `connect`, `:check()`, `:run()` and `:wait()` are all async, so several
  of them proceed concurrently under [`task`](task.md) — see below.
- **The result table matches `proc` exactly** — the same fields, built
  through the same helper so the two cannot drift:

  | Field | Type | When present |
  | --- | --- | --- |
  | `success` | boolean | always |
  | `status` | number | always, but `nil` when the process died of a signal |
  | `signal` | number | only on a signal death (e.g. `15`) |
  | `signal_name` | string | with `signal`, if the number is a named one (e.g. `"SIGTERM"`) |
  | `stdout` / `stderr` | string or buffer | only if that stream was captured |

  A non-zero remote exit is not an error — only a failed connection or a
  failure to start the remote process raises. That includes exit code
  `127` (command not found) and `255` (ssh's own failure code): they come
  back as `status = 127`/`255` with whatever output was captured. A remote
  command that itself exits 255 cannot be told apart from ssh failing.
- **`signal`/`signal_name` describe the *local* `ssh` client, not the
  remote command.** This is worth stating plainly, because the field is
  easy to misread: the status observed here belongs to the `ssh` binary
  running on **this** machine. A remote command killed by a signal on the
  far side does not reach us as a signal at all — sshd reports it as an
  *exit code* (typically `255`, or `128+n` depending on the server), so
  it arrives as `status`. `result.signal` on an ssh result means the
  local `ssh` process itself died of a signal. There is no fix for this
  inside the module: SSH carries no signal channel in either direction
  (which is also why there is no `:kill()`). To learn how a remote
  process died, ask the far side — run it under a remote shell that
  reports `$?`, or check the remote service manager.

  `if result.signal then` is the test for a signal death; `status = nil`
  alone says an exit code is missing without saying why. Both fields
  appear together, and `signal_name` is
  simply absent for a number the table does not name, rather than an
  invented `"SIG37"`.
- **Things SSH cannot do**, each present only to fail with an explanatory
  error (whatever the arguments) rather than being a `nil` method:
  - no `:env()`/`:envs()`/`:env_remove()`/`:env_clear()`, `:current_dir()`
    or `:exec()` — sshd controls the remote environment, not the local
    `ssh` client. Use an explicit remote shell instead:
    `s:command({ "sh", "-c", "cd /srv && VAR=1 exec ./deploy" })`.
  - no `:kill()`/`:signal()` on a remote child, and no `:id()` (there is no
    local pid) — SSH offers no signal channel. Dropping the connection (or
    letting the script end) leaves the remote process running; stop it from
    a second command such as `s:command({ "pkill", "-f", pattern }):run()`.
- **Errors surface where the work happens.** `ssh.connect` raises for an
  unreachable host, auth failure or host-key problem; `:run()`/`:spawn()`
  raise when the remote exec cannot be started; `:wait()` raises only if
  waiting itself fails (a dropped connection), not for a non-zero exit.
  Messages carry the underlying reason, not just "failed to connect".
- **Abandoning a `:wait()`** (e.g. a `task.timeout` around it) does not
  consume the child: the handle still answers a later `:wait()`/`:try_wait()`.

## `ssh.connect`

Opens a session to `host`, using the local ssh config for everything
beyond the name.

**Signature:** `ssh.connect(host: string, options: { tofu: boolean? }?) -> Connection`

**Parameters:**

- `host` — the ssh destination, resolved exactly as `ssh <host>` would
  resolve it (config aliases, `ProxyJump`, `user@host`, `ssh://` URIs).
- `options.tofu` — trust on first use: record an unknown host key instead of
  refusing it. Default `false` (strict).

**Returns:** a `Connection`, once the handshake completes.

**Errors:** if the connection cannot be established — unreachable host,
auth failure, an unknown host key (unless `tofu`), a *changed* host key; the
message includes ssh's reason. Raises if `options.tofu` is not a boolean.

```lua
local s = ssh.connect("prod")
local fresh = ssh.connect("new-host", { tofu = true })
```

## `Connection:close`

Shuts the session (its control master) down, awaited.

**Signature:** `conn:close() -> ()` (yields)

**Returns:** nothing. Closing twice is a no-op; afterwards every other method
on the connection raises `"this ssh session has been closed"`.

**Errors:** raises if a command or child made from this connection is still
alive (closing needs sole ownership; the session stays open), or if the
shutdown itself fails.

Without `:close()`, the master is shut down when the connection is garbage
collected, with a *blocking* `ssh -O exit` at an unpredictable moment.

```lua
local s = ssh.connect("prod")
s:command({ "uptime" }):run()
s:close()
```

## `Connection:host`

The host name the connection was opened for.

**Signature:** `conn:host() -> string`

**Returns:** the same string passed to `ssh.connect`.

```lua
local s = ssh.connect("prod")
print(s:host())   -- "prod"
```

## `Connection:check`

Asks whether the session is still usable — relevant for a long-lived
connection that may have died between commands.

**Signature:** `conn:check() -> (boolean, string?)`

**Returns:** `true` on a healthy session; otherwise `false` plus a
message describing why.

A dead connection is an expected answer here, not a failure, so this
reports rather than raising — no `pcall` needed.

It is `ssh -O check` against the *local* control master, so it is cheap
and does **not** round-trip to the remote host: it answers "is my
multiplexed connection alive", not "is the far side healthy" — a machine
that is up but wedged still passes.

```lua
local s = ssh.connect("prod")
-- ... much later, after other work ...
local ok, err = s:check()
if not ok then
    log.warn("connection to prod died: " .. err)
    s = ssh.connect("prod")   -- reconnect; there is no repair
end
```

To check the remote host itself rather than the connection, run a
command and look at the result:

```lua
local r = s:command({ "true" }):run()
if not r.success then ... end
```

## `Connection:command`

Builds a remote command, finalized with `:run()` or `:spawn()`.

**Signature:** `conn:command(spec: string | {string, ...}) -> RemoteCommand`

**Parameters:**

- `spec` — either a program name (arguments added later via `:arg`/
  `:args`), or a list holding the program and its arguments together:
  `{"systemctl", "restart", "nginx"}`.

**Returns:** a `RemoteCommand` builder. Both streams default to
`proc.NULL` — nothing reaches the terminal unless asked for.

**Methods:** `arg` · `args` · `stdin` · `stdout` · `stderr` · `run` ·
`spawn` · `interactive`; `env`/`envs`/`env_remove`/`env_clear`/
`current_dir`/`exec` exist only to raise.

**Errors:** if `spec` is a table whose first element is missing (empty
list), or neither a string nor a table.

```lua
local cmd = s:command({ "systemctl", "is-active", "nginx" })
-- equivalently:
local cmd = s:command("systemctl"):arg("is-active"):arg("nginx")
```

The program and each argument are shell-escaped, so they arrive on the far
side as exactly those words and nothing is word-split or glob-expanded. (sshd
still runs the result through the remote login shell.) To use shell
features, invoke a shell explicitly:

```lua
s:command({ "sh", "-c", "ls *.txt | wc -l" })
```

## `RemoteCommand:arg` / `:args`

Append one or several arguments to the command being built.

**Signature:** `cmd:arg(value: string) -> RemoteCommand` /
`cmd:args(values: {string, ...}) -> RemoteCommand`

**Returns:** the same builder, so calls chain.

```lua
s:command("journalctl"):args({ "-u", "nginx", "-n", "50" }):run()
```

## `RemoteCommand:stdin`

Set what the command reads. Same sources as `proc`'s [`:stdin`](proc.md):
`proc.NULL` (default), `proc.INHERIT`, `proc.file(path)` (a local file), or
a string or buffer written to the command and then closed.

**Signature:** `cmd:stdin(source: Redirect | string | buffer) -> RemoteCommand`

**Errors:** raises with `"expected a stdin source"` for anything else.

```lua
s:command({ "tee", "/tmp/note" }):stdin("hello\n"):run()
```

## `RemoteCommand:stdout` / `:stderr`

Set where a stream goes. Same targets as `proc`'s
[`:stdout`/`:stderr`](proc.md) — `proc.NULL` (default), `proc.INHERIT`,
`proc.OUT`/`proc.ERR`, `proc.STDOUT`/`proc.STDERR` (alias the other stream,
bound at assignment time; a self-alias raises), `proc.file(path)` (always
local — see above), `proc.buffer()`, `proc.string()`.

**Signature:** `cmd:stdout(target: Redirect) -> RemoteCommand` /
`cmd:stderr(target: Redirect) -> RemoteCommand`

**Returns:** the same builder, so calls chain.

**Errors:** passing the same `proc.file(path)` handle is fine and merges
the streams into one write offset; passing *two separate* `proc.file()`
handles that name the **same path** raises, since that would give each
its own write offset and corrupt the file — reuse one handle instead.

```lua
local r = s:command({ "df", "-h" }):stdout(proc.string()):run()
print(r.stdout)
```

## `RemoteCommand:run`

Spawns the command and waits for it to exit.

**Signature:** `cmd:run() -> table` (yields)

**Returns:** the result table — `success`, `status`, `signal`/
`signal_name` on a signal death of the local `ssh` client, and `stdout`/
`stderr` when captured (see the result table shape above).

**Errors:** if the remote exec itself fails to start (rare — a dead
connection at spawn time, say). A non-zero exit is **not** an error.

```lua
local r = s:command({ "systemctl", "restart", "nginx" }):run()
if not r.success then
    log.warn("restart failed: status " .. tostring(r.status))
end
```

## `RemoteCommand:spawn`

Starts the command without waiting for it, returning a handle to wait on
later.

**Signature:** `cmd:spawn() -> RemoteChild` (yields)

**Returns:** a `RemoteChild`.

**Errors:** same as `:run()` — only a failure to start raises, and it raises
here, not at `:wait()`. The call yields while the remote exec is set up.

Output is drained from spawn onward, so the remote side does not stall on a
full pipe while you do other work.

```lua
local child = s:command({ "long-running-job" }):spawn()
-- ... do other work ...
local r = child:wait()
```

## `RemoteCommand:interactive`

Runs the command on a remote pseudo-terminal attached to this terminal, and
waits for it.

**Signature:** `cmd:interactive() -> table` (yields)

**Returns:** the result table (`success`/`status`, no captures).

**Errors:** raises if `:stdin()`/`:stdout()`/`:stderr()` were configured (the
pty is this terminal), or if the system `ssh` cannot be started. A non-zero
exit is not an error. While it runs, SIGINT/SIGQUIT are ignored here and
reach the remote program through the pty. Implemented as `ssh -tt` over the
existing control master, so it shares the session's authentication.

```lua
s:command({ "top" }):interactive()
```

## `RemoteCommand:env` / `:envs` / `:env_remove` / `:env_clear` / `:current_dir` / `:exec`

Always raise, whatever arguments they get. Present only to fail clearly
rather than being a `nil` method: sshd controls the remote environment and
working directory, not the local `ssh` client, and there is no remote
process image to replace, so there is no equivalent to `proc`'s
`:env()`/`:current_dir()`/`:exec()`.

**Errors:** always, with a message pointing at the workaround — an
explicit remote shell:

```lua
-- does not work:
-- s:command({ "cmd" }):env("VAR", "1")

-- works instead:
s:command({ "sh", "-c", "VAR=1 exec cmd" })
s:command({ "sh", "-c", "cd /path && exec cmd" })
```

## `RemoteChild:wait`

Waits for a spawned command to exit.

**Signature:** `child:wait() -> table` (yields)

**Returns:** the result table, same shape as `:run()`'s. Captured output
is drained concurrently while waiting, so a command that outproduces a
pipe buffer cannot deadlock.

**Errors:** raises if called twice on the same child (it has already
been waited for). An abandoned `:wait()` does not count: the child is given
back and can be waited for again.

```lua
local child = s:command({ "sleep", "5" }):spawn()
local r = child:wait()
```

## `RemoteChild:try_wait`

Checks whether a spawned command has exited, without waiting.

**Signature:** `child:try_wait() -> table | nil` (yields)

**Returns:** `nil` while it is still running, otherwise what `:wait()`
returns (consuming the child).

## `RemoteChild:kill` / `:signal` / `:id`

Always raise. SSH offers no signal channel, so there is no way to signal a
remote process from here, and there is no local pid to report.

**Errors:** always, with a message noting the alternative:

```lua
-- does not work:
-- child:kill()

-- stop it from the remote side with a second command:
s:command({ "pkill", "-f", "long-running-job" }):run()
```

Note that abandoning a remote child (dropping the connection, or letting
the script end without waiting) leaves it running on the remote host.

## Connecting to many hosts at once

`ssh.connect`, `:run()`, `:check()` and `:wait()` all yield rather than
blocking, so fleet-wide work belongs under [`task`](task.md):

```lua
local handles = {}
for _, host in ipairs(hosts) do
    table.insert(handles, task.spawn(function()
        return ssh.connect(host)
            :command({ "uptime" })
            :stdout(proc.string())
            :run()
    end))
end

for i, r in ipairs({ task.await(handles) }) do
    print(hosts[i], r.stdout)
end
```

Thirty hosts that each take a second finish in about a second rather
than thirty. Without `task.spawn`, the connections happen one at a time.

## Bounding how long a remote command may take

An unreachable or wedged host would otherwise hang the run, so a
deadline belongs on remote work more than on local:

```lua
local r = task.timeout("10s", function()
    return ssh.connect(host):command({ "uptime" }):stdout(proc.string()):run()
end)
if r == task.TIMEOUT then
    log.warn(host .. " did not answer in 10s")
end
```

This bounds **how long the script waits**, not how long the remote
command runs: SSH has no signal channel, so an abandoned remote process
keeps going on its host. Where that matters, make the remote side
enforce it too — `s:command({ "timeout", "10", "some-job" })` — or clean
up with a remote `pkill`.
