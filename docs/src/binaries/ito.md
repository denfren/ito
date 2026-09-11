# `ito` — the full toolset

An automation runner. `ito` executes a trusted [Luau](https://luau.org)
script that can read and write files, run local commands, run remote ones
over SSH, query AWS, and overlap all of it concurrently.

```sh
ito deploy.luau                    # run a script file
ito -C /srv/app deploy.luau        # ... from another working directory
ito deploy.luau --env prod -v      # ... with arguments for the script itself
```

The script is trusted: it has the whole standard library, and what it
says, happens. To preview edits or to get a run that depends on nothing
but the files and the script, use [`ied`](ied.md), the restricted variant.

## Scripts as programs

`ito`'s own flags go before the script path. Everything after it belongs
to the script and reaches it verbatim as the global `argv` — including
arguments spelled like `ito`'s own flags, so the `-v` above is the
script's, not `ito`'s. That is what lets a script be a command in its own
right:

```lua
#!/usr/bin/env -S ito
local env = cli.opt("e/env=", "target environment")
local dry = cli.opt("n/dry-run", "report without changing anything")

function main()
    cli.parse()
    print((dry() and "would deploy to " or "deploying to ") .. env())
end
```

```sh
$ chmod +x deploy && ./deploy --env prod -n
$ ./deploy --help          # cli's generated help, not ito's
```

The kernel runs `ito ./deploy --env prod -n`, so every argument after the
script path is the script's, and `--help` reaches
[`cli`](../modules/cli.md) rather than `ito`. A shebang can still set
`ito`'s own flags ahead of the script — `#!/usr/bin/env -S ito -C /srv`
— because they come before the path the kernel appends.

The `#!` line is turned into a Luau comment rather than removed, so line
numbers in error messages match the file.

## The operating model

**`fs` is not confined.** A relative path resolves against the working directory (`-C`), the same way
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
previewable form: `fs.mkdir`, `fs.remove_all`, `fs.rename` and `fs.copy`. A
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

`fs.require_root()`, `fs.require_dir()` and `fs.script()` answer the
other "where am I" questions — the directory imports are confined to, the
one the calling file's own relative imports resolve against, and the
running script itself. A module `require`d
from a subdirectory cannot otherwise spell its own location, the working
directory being `-C` rather than wherever it happens to live:

```lua
-- in <root>/sub/lib.luau
local fixture = path.join(fs.require_dir(), "fixtures/a.json")
local shared  = path.join(fs.require_root(), "lib/shared.yaml")
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
stream — the shell's `1>&2` — name it: `:stdout(proc.ERR)`. To bind a
command's stream to its *other* stream, use `proc.STDOUT`/`proc.STDERR`
(`:stderr(proc.STDOUT)`). `:stdin(...)` feeds input; `:envs`/`:env_remove`
adjust the child's environment.

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
[`fmt`](../modules/fmt.md#styles) styling methods (`("ok"):green()`)
emit ANSI codes when stdout is a terminal and stay silent when the run is
piped, so a report is readable either way with no `if` at the call site:

```lua
print(("ok"):green() .. " deployed")
print(("FAIL"):bold():red() .. " " .. host)
```

`fmt.is_tty()` is the check behind that (`"stderr"`/`"stdin"` on request),
and `fmt.colors(true|false|nil)` overrides it. Detection honours
`NO_COLOR` and `CLICOLOR`/`CLICOLOR_FORCE`.

**Output is vanilla.** `print` writes `tostring` of each argument
(honouring `__tostring`; a plain table prints as `table: 0x…`, not JSON),
tab-joined and newline-terminated, to stdout, flushed on every write. It
is never rerouted to the log, and a closed pipe ends the run quietly with
exit `141`. For stderr or unformatted writes use the `io` handles:

```lua
io.stderr:write("warning: ", 3, " files skipped\n")
io.stdout:write("a", "b"):write("\n")   -- write chains; strings/numbers only
local first = io.read()                 -- one line
local rest  = io.read("a")              -- everything left
for line in io.lines() do print(line) end
```

`:close()` raises and `:setvbuf()` is a no-op. There is no `-q` flag:
`-v` and `-vv` are the only verbosity controls, and they affect logs only.

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
| [`hash`](../modules/hash.md) | MD5/SHA-1/SHA-224/SHA-256/SHA-384/SHA-512 of a string, a buffer or a file |
| [`encoding`](../modules/encoding.md) | base64 (standard, plus an engine builder for other alphabets) |
| [`ansible`](../modules/ansible.md) | Ansible Vault: encrypt, decrypt, load a password file |
| [`json`](../modules/json.md) | parse and emit JSON |
| [`toml`](../modules/toml.md) | parse and emit TOML |
| [`yaml`](../modules/yaml.md) | parse and emit YAML, including format-preserving edits |
| [`hcl`](../modules/hcl.md) | parse and emit HCL, including format-preserving edits |
| [`j2`](../modules/j2.md) | render Jinja2 templates |
| [`re`](../modules/re.md) | regular expressions |
| [`fmt`](../modules/fmt.md) | colours, text helpers, status symbols, aligned tables |
| [`string`](../modules/string.md) | character-aware string helpers (sugar over `fmt`) |
| [`table`](../modules/table.md) | arrays and objects, and the `table.array` marker (plus the global `null`) |
| [`ito`](../modules/ito.md) | the version guard and `ito.slug` |
| [`log`](../modules/log.md) | logging to stderr (`-v`/`-vv` raise the level) |
| [`io`](../modules/io.md) | `io.stdout`/`io.stderr`/`io.stdin` handles (`:write`, `:flush`, `:read`, `:lines`), plus `io.write`/`read`/`lines`/`flush` on the defaults |
| [`ui`](../modules/ui-prompt.md) | interactive prompts ([`ui.prompt`](../modules/ui-prompt.md)) and progress ([`ui.progress`](../modules/ui-progress.md)) |
| [`aws.cloudtrail`](../modules/aws-cloudtrail.md) | look up CloudTrail API call history |
| [`aws.cloudwatch`](../modules/aws-cloudwatch.md) | read CloudWatch metrics |
| [`cli`](../modules/cli.md) | declare the script's own options and arguments, with a generated `--help` |

Print any of them with `--doc <topic>`, e.g. `ito --doc proc`.

## Gotchas

**`ito`'s flags go before the script.** `ito deploy.luau -C /srv` hands
`-C /srv` to the script; it does not change the working directory. Write
`ito -C /srv deploy.luau`.

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
`os.getenv` is absent under `ied`, so a script meant to run
there too should test `if os.getenv then`.

**SSH has no `env()`/`envs()`, `current_dir()`, `exec()` or `kill()`/`signal()`.**
They raise rather than being absent. The remote environment belongs to
sshd, and SSH offers no signal channel — dropping the connection leaves the
remote process running. Use an explicit remote shell for the first two.

**SSH host keys are checked strictly.** An unknown or changed key is an
error; opt into trust-on-first-use with `ssh.connect(host, { tofu = true })`.
Exit codes `127` and `255` come back as a result, not a raise.

**An unawaited task still runs.** Tasks are drained before the run ends,
so one spawned purely for its side effects completes. A task that fails
without ever being awaited is logged and fails the run.

**A leading `/` is the host root.** `fs.write("/config.toml")` writes to
the filesystem root, not to `<workdir>/config.toml`. Write the path
without the leading slash for one relative to the working directory.

**Exit codes.** `0` success, `2` a failure that is not the script's fault
(usage, environment), `3` a failure in the script itself (a parse or
runtime error), `141` the output pipe went away (`| head`; the run was cut
short, like a process killed by SIGPIPE).
