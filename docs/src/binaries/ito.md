# `ito` — the full toolset

An automation runner. `ito` executes a trusted [Luau](https://luau.org)
script that can read and write files, run local commands, run remote ones
over SSH, query AWS, and overlap all of it concurrently.

```sh
ito deploy.luau               # run a script file
ito deploy.luau -C /srv/app   # ... from another working directory
ito -s 'proc.command({"uname","-a"}):stdout(proc.INHERIT):run()'
ito deploy.luau hosts.json    # ... with a file handed to it as `input`
ito deploy.luau :: --env prod   # ... with arguments for the script itself
```

The script is trusted: there is no sandbox to opt out of, and no preview
mode. What the script says, happens.

## The operating model

**`fs` is the real filesystem.** There is no sandbox around it. A
relative path resolves against the working directory (`-C`), the same way
it would for any other program run from there; an absolute path means
what it says, and `..` climbs out:

```lua
fs.read("config.toml")        -- <workdir>/config.toml
fs.read("../shared/base.toml")
fs.read("/etc/hosts")
```

`-C` sets *where relative paths start*, not a boundary — a script that
can already spawn `rm` is not confined by one.

Traversals (`fs.list`, `fs.walk`, `fs.glob`, `fs.finder`) report paths
**relative to the working directory**, the same shape `ls` or `find .`
gives. A path that genuinely lies outside it — from a `..` or absolute
start directory — comes back absolute, there being no shorter honest name
for it:

```lua
fs.glob("**/*.toml")   -- { "config.toml", "sub/db.toml" }
fs.list("/etc")        -- { "/etc/hosts", ... }
```

**Writes land immediately.** `fs.write` and `fs.remove` take effect as
they are called, so a script reads back its own writes and a subprocess
sees them:

```lua
fs.write("config.toml", "updated = true")
fs.read("config.toml")               -- "updated = true"
proc.command({ "cat", "config.toml" }):stdout(proc.INHERIT):run()
```

This is the honest model once a script can also spawn subprocesses:
holding writes back would promise that nothing changed until the script
returned, and a single `proc` or `ssh` call touching a written file
breaks that promise. Nothing is held back, so there is nothing to preview
or roll back.

The same reasoning gives `ito` the filesystem operations that have no
previewable form: `fs.mkdir`, `fs.rmdir`, `fs.rename` and `fs.copy`. A
write makes its own parent directories, so a script that only writes
never needed them — but a child process does, and creating a directory
for one should not mean shelling out to `mkdir`:

```lua
fs.mkdir("build/out")
proc.command({ "make" }):current_dir("build/out"):run()
```

`fs.cwd()` belongs to the same group: it returns the `-C`-resolved
working directory as an absolute path, which is what a child process or a
remote command usually needs to be told. Traversals report paths relative
to that directory, so this is the one path with no relative form.

`ito.require_root()` and `ito.require_dir()` answer the other two "where
am I" questions — the directory imports are confined to, and the one the
calling file's own relative imports resolve against. A module `require`d
from a subdirectory cannot otherwise spell its own location, the working
directory being `-C` rather than wherever it happens to live:

```lua
-- in <root>/sub/lib.luau
local fixture = path.join(ito.require_dir(), "fixtures/a.json")
local shared  = path.join(ito.require_root(), "lib/shared.yaml")
```

**Commands are built, then run.** `proc.command` and
`ssh.connect(…):command` return a builder; `:run()` starts it and waits,
`:spawn()` starts it and hands back a child to wait for later.

```lua
local r = proc.command({ "git", "rev-parse", "HEAD" })
    :stdout(proc.string())
    :run()
print(r.stdout)
```

**Command output is discarded unless you ask for it.** Both streams
default to `proc.NULL`, so a command never scribbles on your terminal by
accident. Capture it with `proc.string()` / `proc.buffer()`, send it to a
file with `proc.file(<path>)`, or pass it through with `proc.INHERIT`.
Handing the *same* handle to both `:stdout()` and `:stderr()` merges
them, exactly as `2>&1` does. To put a stream on *this* process's other
stream — the shell's `1>&2` — name it: `:stdout(proc.ERR)`.

**A child can be signalled, not just killed.** `child:kill()` sends
SIGKILL and reaps the child; `child:signal(<sig>)` sends any signal and
leaves it running, which is what makes the polite shutdown expressible:

```lua
local child = proc.command({ "long-job" }):spawn()
child:signal("TERM")                    -- or proc.SIGTERM, or 15
task.wait("5s")
child:signal("KILL")                    -- it had its chance
local result = child:wait()
```

Signalling an already-exited child is harmless here — the child is not
reaped until `:wait()`, so the pid is still the one that was spawned.

A child that died of a signal reports `result.signal` and
`result.signal_name` alongside the `status = nil` that says there is no
exit code.

**Concurrency is opt-in and cooperative.** A plain call runs to
completion before the next statement. `task.spawn` puts one on its own
thread so several are in flight at once:

```lua
local handles = {}
for _, host in ipairs({ "a", "b", "c" }) do
    handles[#handles + 1] = task.spawn(function()
        return ssh.connect(host):command({ "uptime" }):stdout(proc.string()):run()
    end)
end
local a, b, c = task.await(handles)
```

This is concurrency, not parallelism: tasks share one thread and
interleave only where they wait on I/O — which is all that I/O-bound work
needs.

**`main` is the entry point.** After the script body runs, a global
`main` function is called if one is defined. A file loaded with `require`
has its body evaluated but its `main` left alone, so a script with its
work inside `main` can be both run and imported.

**The environment is readable.** `os.getenv(<name>)` returns a variable's
value, or `nil` when it is unset:

```lua
local region = os.getenv("AWS_REGION") or "eu-central-1"
```

A variable set to the empty string reads as `""`, not `nil`, so the two
stay distinguishable — use `or` for a default only when an empty value
should fall through to it. There is no setter; see the gotcha below.

**Output can be coloured, and knows when not to be.** The
[`string`](../modules/string.md#terminal-styling-ansi) styling methods
emit ANSI codes when stdout is a terminal and stay silent when the run is
piped, so a report is readable either way with no `if` at the call site:

```lua
print(("ok"):green() .. " deployed")
print(("FAIL"):bold():red() .. " " .. host)
```

`ito.is_tty()` is the check behind that (`"stderr"`/`"stdin"` on request),
and `ito.colors(true|false|nil)` overrides it. Detection honours
`NO_COLOR` and `CLICOLOR`/`CLICOLOR_FORCE`.

**Data can arrive on the command line.** An optional `INPUT` positional —
a file path, or `-` for stdin — reaches the script as the global `input`,
which is what makes a script usable as a stage in a pipeline:

```sh
ito deploy.luau hosts.json          # input = the file's text
aws-inventory | ito deploy.luau -   # input = stdin
```

`input` is **raw text**, never parsed for you: a file's extension is a
hint rather than a fact, and stdin carries no indication of its format, so
the script says which parser it wants.

```lua
for _, host in ipairs(json.parse(input).hosts) do
    ssh.command(host, { "systemctl", "restart", "app" }):run()
end
```

Omit the positional and `input` is nil, which is distinct from an empty
string — a script can tell "nothing was passed" from "something empty was
passed".

## Modules

| Module | For |
| --- | --- |
| [`fs`](../modules/fs.md) | read, write and remove files anywhere on the host |
| [`path`](../modules/path.md) | join, split and inspect paths as strings — no disk access |
| [`time`](../modules/time.md) | timestamps, duration deltas, and the clock |
| [`proc`](../modules/proc.md) | run local commands, and the [redirect targets](../modules/redirect.md) for their output |
| [`ssh`](../modules/ssh.md) | run commands on another machine |
| [`aws.config`](../modules/aws-config.md) | build an AWS client for an explicit region, profile or endpoint |
| [`aws.ec2`](../modules/aws-ec2.md) | query AWS EC2, and start, stop or terminate instances |
| [`aws.s3`](../modules/aws-s3.md) | read buckets and objects from AWS S3 |
| [`aws.autoscaling`](../modules/aws-autoscaling.md) | read and drive Auto Scaling groups, including instance refreshes |
| [`task`](../modules/task.md) | overlap any of the above concurrently |
| [`hash`](../modules/hash.md) | MD5/SHA-1/SHA-256/SHA-512 of a string, a buffer or a file |
| [`json`](../modules/json.md) | parse and emit JSON |
| [`toml`](../modules/toml.md) | parse and emit TOML |
| [`yaml`](../modules/yaml.md) | parse and emit YAML, including format-preserving edits |
| [`hcl`](../modules/hcl.md) | parse and emit HCL, including format-preserving edits |
| [`j2`](../modules/j2.md) | render Jinja2 templates |
| [`re`](../modules/re.md) | regular expressions |
| [`string`](../modules/string.md) | character-aware string helpers, ANSI styling |
| [`table`](../modules/table.md) | arrays and objects |
| [`ito`](../modules/ito.md) | the version guard, the `null` sentinel, and the colour switch |
| [`log`](../modules/log.md) | logging to stderr |
| [`io`](../modules/io.md) | raw, unfiltered stdout/stderr writes |
| [`cli`](../modules/cli.md) | declare the script's own options and arguments, with a generated `--help` |

Print any of them with `--doc <topic>`, e.g. `ito --doc proc`.

## Gotchas

**`SCRIPT` and `INPUT` cannot both be `-`.** Stdin can only be read once.
Use `-s` for an inline script, or name a file for one of the two.

**`INPUT` is not a `cli.arg()`.** The input file is `ito`'s own
positional, matched before `::`, so a script's
[`cli`](../modules/cli.md) declarations never see it — `cli.arg("file")`
reads whatever came *after* `::`, which is a different thing. The data
arrives as `input`, as always.

**A failing command is not an error.** `:run()` returns a result whose
`success` and `status` describe how the command exited; a non-zero exit
does not raise. Check it yourself:

```lua
local r = proc.command({ "test", "-f", "/etc/hosts" }):run()
if not r.success then error("missing") end
```

**Redirect targets are single-use handles.** Each call to
`proc.string()` makes a *distinct* handle. Two separate `proc.file()`
handles on one path is an error rather than silent corruption — each
would carry its own write offset. Reuse one handle to merge instead.

**`proc.file(<path>)` is always local**, even for an `ssh` command: the
target is a file descriptor on this machine, so remote output travels
back over the connection to be written here. To write on the far side,
redirect inside the remote command itself.

**`os.getenv` reads; nothing writes.** There is no `os.setenv`. To give a
subprocess a variable, set it on that command — `proc.command({…}):env(
"KEY", "value")` — which scopes it to the child instead of mutating a
process-wide table that every later task would then see. Note also that
`os.getenv` is the one key on `os` that only `ito` has, so a script meant
to run under `iq` or `ied` too should test `if os.getenv then`.

**SSH has no `env()`, `current_dir()` or `kill()`.** The remote
environment belongs to sshd, and SSH offers no signal channel — dropping
the connection leaves the remote process running. Use an explicit remote
shell for the first two.

**An unawaited task still runs.** Tasks are drained before the run ends,
so one spawned purely for its side effects completes. A task that fails
without ever being awaited is logged and fails the run.

**A leading `/` is the host root.** `fs.write("/config.toml")` writes to
the filesystem root, not to `<workdir>/config.toml`. Write the path
without the leading slash for one relative to the working directory.

**Exit codes.** `0` success, `2` a failure that is not the script's fault
(usage, environment), `3` a failure in the script itself (a parse or
runtime error).
