# ito

A [Luau](https://luau.org) script runner with a small standard
library for inspecting, generating and editing config files. Scripts get
a filesystem module (`fs`), plus `path`, `json`, `toml`, `yaml`, `hcl`,
and `j2` modules for path manipulation, parsing, emitting structured
data, and rendering templates.

The only relevant path is the working directory (override with `-C`),
which is what relative `fs` paths resolve against. Under `iq` and `ied`
it is also a boundary — `fs` is a virtual filesystem rooted there, and a
path escaping it is an error. Under `ito` it is a starting point only:
`fs` is the real filesystem. File I/O happens through `fs` either way.

Exit codes: `0` success, `1` success with changes (only where
`-e`/`--exit-code` applies), `2` a generic failure (usage, environment,
output), and `3` a failure in the script itself (parse or runtime error).

There is no built-in reaction to Ctrl-C, or to any other signal: a signal
nothing has caught keeps its default disposition, so an interrupted run ends
the way it always would without ito running at all — outstanding tasks are
not drained and buffered writes are not flushed, so an interrupted `ied
--write` leaves the file tree untouched; writes `ito` has already made are on
disk and stay. Child processes started by `proc` are in the same process
group, so a terminal Ctrl-C reaches them directly and they stop too; a `kill
-INT` sent to the program alone leaves them running, as does a remote `ssh`
command, SSH having no signal channel. `proc.trap` (`ito`/`irun` only) is the
one place a script opts into reacting to a signal sent to itself — see
[`proc`](./modules/proc.md).

## Three programs

The same script runner ships under several names, three of which differ in
what a script is allowed to reach. Pick the least capable one that does the job —
the boundary is enforced by the missing globals, not by a policy check, so
`iq` genuinely cannot write and `ied` genuinely cannot run a command.

| | For |
| --- | --- |
| [`iq`](binaries/iq.md) | querying files: text in, text out, nothing changed |
| [`ied`](binaries/ied.md) | editing files: buffered changes you can diff, list, or apply |
| [`ito`](binaries/ito.md) | automation: direct writes, commands, SSH, AWS, concurrency |

A fourth name, [`irun`](binaries/irun.md), is not a fourth tier: it is
`ito`'s capability with `ito`'s flags removed, so that a `#!/usr/bin/env
-S irun` script receives the whole command line as its own arguments.

Each has its own chapter covering how it works and what it exposes. They
share this documentation, so `--doc <topic>` reaches every topic from any
of them (`irun` parses no flags, so use `ito --doc`).

The pages below apply to all of them. Where behaviour depends on the
program — chiefly how `fs` writes reach disk, and whether `fs` is rooted
at the working directory or is the host filesystem — the chapter says so.

## Running scripts

```sh
ito <path>          # run a script file
ito -               # read a script from stdin
ito -s '<expr>'     # run an inline expression
ito                 # list the scripts in the working directory
ito <path> :: <arg>...   # pass arguments to the script (see args)
```

`SCRIPT` (the positional) is a **path** — nothing else. There is no name
lookup and no search list, so `ito deploy` means the file `deploy` and
fails if that file does not exist. Running `ito` with no arguments lists
the `.lua`/`.luau` files it finds in `./`, `ito/` and `.ito/`, printed as
the command you would type.

A bare `::` ends the program's own arguments: everything after it is the
**script's**, handed through untouched as the global `argv`. That is the
only way to pass a script arguments, and it means a script's flags never
have to avoid colliding with the program's.

```sh
ito deploy.luau -v :: --env prod    # -v is ours, --env prod is the script's
```

See [args](args.md) for what `argv` looks like.

`-C` applies **after** every path on the command line has been resolved.
Paths you write (`SCRIPT`, `-r`) therefore mean what they mean where you
are standing, while `-C` moves the working directory `fs` resolves
relative paths against:

```sh
ito ./gen.lua -C build     # runs gen.lua from here, writes into build/
```

This is also what makes a shebang script work: the kernel passes it as
`./<script>`, so it is still found after its own
`#!/usr/bin/env -S ito -C <dir>` line moves the working directory.

## Imports

A script file imports relative to its own directory, confined to that
tree (see [require](./luau/require.md)).

A script from stdin or `-s` has no directory of its own, so it **cannot
import at all** unless `-r <dir>` names one:

```sh
echo 'require("./lib")' | ito -            # error: imports are disabled
echo 'require("./lib")' | ito - -r .       # imports from the current directory
```

`-r` is not accepted alongside a `SCRIPT` file, which already has a root.

After the script body runs top to bottom, a global `main` function is
called if the script defines one — see
[Functions](luau/functions.md#the-main-function).

## Help from the CLI

This documentation is embedded in each program, and every one with a
`--doc` carries all of it (`irun` has no flags, so ask `ito`):

```
ito --doc        # this program's overview, then the topic list
ito --doc fs     # print one topic
ito --doc all    # the full reference in one output
```

`--help` / `-h` covers the command-line flags themselves.
