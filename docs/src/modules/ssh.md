# `ssh` — running commands remotely

Host-only (`ito`/`irun`); not available in `iq`/`ied`.

| Function | Signature | Returns |
| --- | --- | --- |
| `ssh.connect` | `(<host>)` | a connection (yields) |

| Connection method | Signature | Returns |
| --- | --- | --- |
| `:host` | `()` | the host it was opened for |
| `:check` | `()` | `true`, or `false` and the reason (yields) |
| `:command` | `(<program>)` or `(<list>)` | a remote command builder |

| Command builder method | Signature | Effect |
| --- | --- | --- |
| `:arg` | `(<value>)` | append one argument |
| `:args` | `(<list>)` | append several arguments |
| `:stdout` | `(<target>)` | where stdout goes (default `proc.NULL`) |
| `:stderr` | `(<target>)` | where stderr goes (default `proc.NULL`) |
| `:run` | `()` | the result, after the command exits (yields) |
| `:spawn` | `()` | a remote child, awaited with `:wait()` (yields) |
| `:env` | — | always raises: no remote env access |
| `:current_dir` | — | always raises: no remote cwd |

| Child method | Signature | Returns |
| --- | --- | --- |
| `:wait` | `()` | the result, after the child exits (yields) |
| `:kill` | — | always raises: SSH has no signal channel |

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

- **Authentication is entirely the ssh config's job.** There is no
  password/key/agent option on `connect` — whatever `~/.ssh/config`
  (or system config) says for that host name is what runs, exactly as
  typing `ssh <host>` would.
- **Host keys are trust-on-first-use.** An unknown host is recorded, a
  *changed* key is an error, and ssh is never allowed to prompt (the
  underlying crate always passes `BatchMode=yes`). This is passed as an
  `-o` override, so it wins over `StrictHostKeyChecking` in the ssh
  config. A host whose key legitimately rotates needs
  `UserKnownHostsFile /dev/null` in its config so every connect looks
  like a first connect.
- **One connection, one control master.** Each `ssh.connect()` is an
  independent session with its own control master, so several
  connections — to the same host or different ones — coexist, and every
  command on one session reuses its single handshake rather than paying
  for a new one.
- **Redirect targets are shared with `proc`.** `:stdout()`/`:stderr()`
  take the exact same values as `proc`'s — `proc.NULL`, `proc.INHERIT`,
  `proc.STDOUT`/`proc.STDERR`, `proc.file(path)`, `proc.buffer()`,
  `proc.string()` — see [`proc`](proc.md) for what each means and how
  merging works. `proc.file(path)` always names a file on **this**
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
  failure to start the remote process raises.
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
  used to be the only indication, which said an exit code was missing
  without saying why. Both fields appear together, and `signal_name` is
  simply absent for a number the table does not name, rather than an
  invented `"SIG37"`.
- **Two things SSH cannot do**, each failing with an explanatory error
  rather than pretending to work:
  - no `:env()` or `:current_dir()` — sshd controls the remote
    environment, not the local `ssh` client. Use an explicit remote
    shell instead: `s:command({ "sh", "-c", "cd /srv && VAR=1 exec ./deploy" })`.
  - no `:kill()` on a remote child — SSH offers no signal channel.
    Dropping the connection (or letting the script end) leaves the
    remote process running; use a remote `pkill`, or run the command
    under a pty.

## `ssh.connect(host)`

Opens a session to `host`, using the local ssh config for everything
beyond the name.

**Signature:** `ssh.connect(host: string) -> Connection`

**Parameters:**

- `host` — the ssh destination, resolved exactly as `ssh <host>` would
  resolve it (config aliases, `ProxyJump`, etc. all apply).

**Returns:** a `Connection`, once the handshake completes.

**Errors:** if the connection cannot be established — unreachable host,
auth failure, a *changed* host key. (A first-seen host key is accepted
automatically, not an error.)

```lua
local s = ssh.connect("prod")
```

## `Connection:host()`

The host name the connection was opened for.

**Signature:** `conn:host() -> string`

**Returns:** the same string passed to `ssh.connect`.

```lua
local s = ssh.connect("prod")
print(s:host())   -- "prod"
```

## `Connection:check()`

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

## `Connection:command(spec)`

Builds a remote command, finalized with `:run()` or `:spawn()`.

**Signature:** `conn:command(spec: string | {string, ...}) -> RemoteCommand`

**Parameters:**

- `spec` — either a program name (arguments added later via `:arg`/
  `:args`), or a list holding the program and its arguments together:
  `{"systemctl", "restart", "nginx"}`.

**Returns:** a `RemoteCommand` builder. Both streams default to
`proc.NULL` — nothing reaches the terminal unless asked for.

**Errors:** if `spec` is a table whose first element is missing (empty
list), or neither a string nor a table.

```lua
local cmd = s:command({ "systemctl", "is-active", "nginx" })
-- equivalently:
local cmd = s:command("systemctl"):arg("is-active"):arg("nginx")
```

Arguments are passed as an argv vector over the remote exec, never
through a shell, so nothing is word-split or glob-expanded on the far
side either. To use shell features, invoke a shell explicitly:

```lua
s:command({ "sh", "-c", "ls *.txt | wc -l" })
```

## `RemoteCommand:arg(value)` / `:args(list)`

Append one or several arguments to the command being built.

**Signature:** `cmd:arg(value: string) -> RemoteCommand` /
`cmd:args(values: {string, ...}) -> RemoteCommand`

**Returns:** the same builder, so calls chain.

```lua
s:command("journalctl"):args({ "-u", "nginx", "-n", "50" }):run()
```

## `RemoteCommand:stdout(target)` / `:stderr(target)`

Set where a stream goes. Same targets as `proc`'s
[`:stdout`/`:stderr`](proc.md) — `proc.NULL` (default), `proc.INHERIT`,
`proc.STDOUT`/`proc.STDERR` (alias the other stream, bound at assignment
time), `proc.file(path)` (always local — see above), `proc.buffer()`,
`proc.string()`.

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

## `RemoteCommand:run()`

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

## `RemoteCommand:spawn()`

Starts the command without waiting for it, returning a handle to wait on
later.

**Signature:** `cmd:spawn() -> RemoteChild` (yields)

**Returns:** a `RemoteChild`.

**Errors:** same as `:run()` — only a failure to start raises.

```lua
local child = s:command({ "long-running-job" }):spawn()
-- ... do other work ...
local r = child:wait()
```

## `RemoteCommand:env()` / `:current_dir()`

Always raise. Present only to fail clearly rather than silently doing
nothing: sshd controls the remote environment and working directory,
not the local `ssh` client, so there is no equivalent to `proc`'s
`:env()`/`:current_dir()`.

**Errors:** always, with a message pointing at the workaround — an
explicit remote shell:

```lua
-- does not work:
-- s:command({ "cmd" }):env("VAR", "1")

-- works instead:
s:command({ "sh", "-c", "VAR=1 exec cmd" })
s:command({ "sh", "-c", "cd /path && exec cmd" })
```

## `RemoteChild:wait()`

Waits for a spawned command to exit.

**Signature:** `child:wait() -> table` (yields)

**Returns:** the result table, same shape as `:run()`'s. Captured output
is drained concurrently while waiting, so a command that outproduces a
pipe buffer cannot deadlock.

**Errors:** raises if called twice on the same child (it has already
been waited for).

```lua
local child = s:command({ "sleep", "5" }):spawn()
local r = child:wait()
```

## `RemoteChild:kill()`

Always raises. SSH offers no signal channel, so there is no way to
signal a remote process from here.

**Errors:** always, with a message noting the alternative:

```lua
-- does not work:
-- child:kill()

-- run it under a pty and Ctrl-C-equivalent yourself, or from the remote side:
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
