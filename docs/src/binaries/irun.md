# `irun` — scripts as programs

A script runner for the `#!` line. `irun` executes a trusted
[Luau](https://luau.org) script with the full standard library — files,
local commands, SSH, AWS, concurrency — and claims **exactly one
argument**: the script path. Everything after it belongs to the script.

```sh
irun deploy.luau                  # run a script
irun deploy.luau --env prod       # argv = {"--env", "prod"}
irun deploy.luau -C /srv -v       # argv = {"-C", "/srv", "-v"} -- not flags
```

Its reason to exist is the shebang. A script written this way is a
program whose interface is entirely its own:

```lua
#!/usr/bin/env -S irun

local force = cli.opt("f/force", "skip the confirmation")
local env = cli.arg("env", "environment to deploy to")

function main()
    cli.parse()
    print("deploying to " .. env())
end
```

```sh
$ chmod +x deploy.luau
$ ./deploy.luau prod --force
deploying to prod
$ ./deploy.luau --help
Arguments: [options] <env>
...
```

## The operating model

**`irun` has no options.** Not `-C`, not `-v`, not `--doc`, not `-s`.
This is the whole design, not an omission: a runner that claimed even one
spelling would be a runner your script could never use that spelling
itself. `./backup.luau -C /srv` has to reach the script, so nothing is
intercepted — there is no `--` or `::` to remember, and no argument that
means something different depending on where it sits.

The command line is therefore positional and total:

| Position | Belongs to | Meaning |
| --- | --- | --- |
| first | `irun` | the script path |
| everything after | the script | `argv`, verbatim |

**The arguments arrive as `argv`.** A plain list of strings, in order,
with the script's first argument at `argv[1]` — the program name is not
element 1, so `#argv` is simply how many were passed. A script given none
sees an empty table, never nil. Nothing is parsed *into* it: `--force` and
`prod` are both just strings, in the order given (see [passing values to
scripts](../args.md)).

**Parsing them is [`cli`](../modules/cli.md).** Declare the options and
arguments the script takes, call `cli.parse()`, and `--help` is written
for you — which is what makes a shebang script a program rather than a
list of strings someone has to remember the order of. `argv` stays raw
beside it for a script that wants its own grammar.

**Everything else is `ito`.** Same runtime, same trust model, same
standard library, same exit codes. `fs` is the real filesystem, writes
land as they happen, and `proc`/`ssh`/`aws`/`task`/`os.getenv` are all
present. Read the [`ito` chapter](ito.md) for how those behave; none of
it differs here.

**Relative paths resolve against the invocation directory.** With no
`-C`, there is nothing to move the working directory, so a script sees the
directory it was called from — the same one a shell script would. A script
that must run somewhere specific says so:

A script that wants files kept *next to itself* rather than next to
whoever ran it asks where it lives: `ito.require_dir()` is the calling
file's own directory, `ito.require_root()` the tree its imports are
confined to.

```lua
local template = fs.read(path.join(ito.require_dir(), "templates/unit.j2"))
```

```lua
proc.command({ "make" }):current_dir("/srv/app"):run()
fs.write(path.join("/srv/app", "build.stamp"), "ok")
```

## Modules

Identical to `ito` — see [its module table](ito.md#modules), or list the
topics with `ito --doc`. `irun` embeds the same documentation but has no
flag to print it, so reach for `ito --doc <topic>`.

## Gotchas

**There is no `--doc` here.** Asking `irun --doc fs` runs a script called
`--doc`, which almost certainly does not exist. Use `ito --doc fs`.

**A bare `irun` is a usage error, not a listing.** The other binaries
list the scripts they can find when invoked with nothing, because
answering "what can I run?" is useful there. Someone reaching for `irun`
already has one specific file in mind — it is the shebang line of it.

**`env -S` is required in the shebang**, as shown above. A bare
`#!/usr/bin/env irun` works on Linux only by accident of single-argument
shebang parsing; `-S` is what makes it portable, and it costs nothing.

**The script path is resolved from the caller's directory.** A shebang
exec hands `irun` the path the kernel used (`./deploy.luau`), which
resolves where the user ran it — so an executable script found through
`$PATH` works, and so does `../tools/deploy.luau`.

**Prefer `ito` when a human types the command.** `irun` optimizes for the
script owning its interface, which is what a `#!` line wants. At an
interactive prompt, `ito -s`, `-C` and `--doc` are worth having, and
`ito script.luau :: --env prod` passes arguments just as well.

**Exit codes.** `0` success, `2` a failure that is not the script's fault
(no script argument, a path that does not exist), `3` a failure in the
script itself (a parse or runtime error). The same as `ito`.
