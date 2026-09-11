# ito

A [Luau](https://luau.org) script runner with a small standard
library for inspecting, generating and editing config files. Scripts get
a filesystem module (`fs`), plus `path`, `json`, `toml`, `yaml`, `hcl`,
and `j2` modules for path manipulation, parsing, emitting structured
data, and rendering templates.

The working directory (override with `-C`) is where relative `fs` paths
start. `..` and absolute paths reach outside it. File I/O happens through `fs`. (Under `ied` the directory is also a
boundary — see [Two programs](#two-programs).)

Exit codes: `0` success, `1` success with changes (only where
`-e`/`--exit-code` applies), `2` a generic failure (usage, environment,
output), `3` a failure in the script itself (parse or runtime error), and `141` the
output pipe went away (`| head`): the run was cut short, like a process
killed by SIGPIPE.

There is no built-in reaction to Ctrl-C, or to any other signal: a signal
nothing has caught keeps its default disposition, so an interrupted run ends
the way it always would without ito running at all — outstanding tasks are
not drained and buffered writes are not flushed, so an interrupted `ied
--write` leaves the file tree untouched; writes `ito` has already made are on
disk and stay. Child processes started by `proc` are in the same process
group, so a terminal Ctrl-C reaches them directly and they stop too; a `kill
-INT` sent to the program alone leaves them running, as does a remote `ssh`
command, SSH having no signal channel. `proc.trap` (absent under `ied`) is the
one place a script opts into reacting to a signal sent to itself — see
[`proc`](./modules/proc.md).

## Two programs

[`ito`](binaries/ito.md) is the program: it runs a script with the full
standard library — files, commands, SSH, AWS, concurrency, the terminal.

[`ied`](binaries/ied.md) is a restricted variant of it, for editing files.
Writes are buffered so the changes can be diffed, listed, or applied, and
a run is **reproducible**: given the same work-directory state and the same
script it always does the same thing. To keep that promise it lacks
everything that would let anything else in — the clock, the environment,
randomness, commands, the network, paths outside the work directory. The
restriction is enforced by the missing globals, not by a policy check, so
`if proc then` is a usable feature test. See
[Reproducibility](binaries/ied.md#reproducibility) for the full list.

These docs describe `ito`; only what is *unavailable or different* in `ied`
is called out, where it applies. Both programs embed all the documentation,
so `--doc <topic>` reaches every topic from either of them.

## Running scripts

```sh
ito [<flags>] <script> [<args>...]   # run a script file, passing it <args>
ito                                  # list the scripts in the working directory
```

The script is a **path** — nothing else. There is no name lookup and no
search list, so `ito deploy` means the file `deploy` and fails if that
file does not exist. Running `ito` with no arguments lists the
`.lua`/`.luau` files it finds in `./`, `ito/` and `.ito/`, printed as the
command you would type.

The program's own flags go **before** the script path. Everything after
it is the **script's**, handed through untouched as the global `argv` —
including arguments spelled like the program's own flags:

```sh
ito -v deploy.luau --env prod -v    # the first -v is ours, the rest is the script's
```

See [args](args.md) for what `argv` looks like.

`-C` applies **after** the script path has been resolved. The path you
write therefore means what it means where you are standing, while `-C`
moves the working directory `fs` resolves relative paths against:

```sh
ito -C build ./gen.lua     # runs gen.lua from here, writes into build/
```

This is also what makes a shebang script work. A script starting with
`#!/usr/bin/env -S ito` owns its whole command line: the kernel runs
`ito ./<script> <args>...`, so every argument lands in `argv`. Flags in
the shebang itself still reach the program — `#!/usr/bin/env -S ito -C
<dir>` sets the working directory, and the script, passed as
`./<script>`, is still found after it moves. The `#!` line is read as a
Luau comment, so line numbers in errors match the file.

## Imports

A script imports relative to its own directory, confined to that tree
(see [require](./luau/require.md)).

After the script body runs top to bottom, a global `main` function is
called if the script defines one — see
[Functions](luau/functions.md#the-main-function).

## Help from the CLI

This documentation is embedded in each program, and each carries
all of it:

```
ito --doc        # this program's overview, then the topic list
ito --doc fs     # print one topic
ito --doc all    # the full reference in one output
```

`--help` / `-h` covers the command-line flags themselves.
