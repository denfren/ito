# ito - infrastructure tool

A [Luau](https://luau.org) script runner with additional modules for IaC repositories.

## Quickstart

**install from source**

```sh
cargo install --path ito
```

Try a one-liner. `ito -s`/`--script` runs a script you type right on the
command line. Here it reads this project's `Cargo.toml` and prints the version:

```sh
# Relative paths resolve against $PWD, which `-C` moves
ito -s 'print(toml.parse(fs.read("ito/Cargo.toml"))["package"].version)'
```

## How it works

Relative paths in ito's own stdlib (`fs`, `j2`, …) resolve against the current work directory, which can be
changed with `-C/--directory <path>` (like in `make`). Under `iq` and `ied` that directory is also a boundary:
`fs` is a virtual filesystem rooted there, and a path escaping it is an error. Under `ito` it is only a starting
point — `fs` is the real filesystem, so `..` and absolute paths reach out. Either way traversals report paths
relative to that directory; only a path that genuinely lies outside it comes back absolute.

You provide a script file that gets executed. The script file can split itself across other `.lua`/`.luau` files with plain Luau `require`, resolved relative to its own directory
and **confined to that tree** — a `require("../../anything.lua")` that climbs out is an error, not a read. An
`@alias/path` import resolves through the `aliases` table of the nearest `.luaurc`, and is confined the same way.
A script from `-s` or stdin has no directory of its own, so it cannot import at all unless `-r/--require <dir>`
names a root for it.

All three flag-parsing programs take an optional `INPUT` positional after the script — a file path, or `-` for
stdin — handed to the script as the global `input` (`ito deploy.luau hosts.json`, `... | ied fix.luau - -d`). It
is raw text: a file's extension is a hint rather than a fact and stdin carries none at all, so the script picks
the parser (`json.parse(input)`). Omit it and `input` is `nil`, which stays distinct from an empty string.
`SCRIPT` and `INPUT` cannot both be `-`, since stdin is only readable once.

A bare `::` ends `ito`'s own arguments: everything after it belongs to the script and arrives as the global `argv`
(`ito deploy.luau -v :: --env prod`). Nothing is interpreted — no option map, no types — so a script parses its
arguments however it likes, and its flags can never collide with the program's. `argv[1]` is the first argument
given, not the program name, and a run without `::` sees an empty table.

For a script that should own its command line outright — a `#!/usr/bin/env -S irun` script, above all — `irun` is
`ito` with every flag of its own removed. It takes one argument, the script path, and everything after it reaches
the script as `argv` with no `::` needed, so `./deploy.luau --env prod -C x` passes all four strings through
untouched. Same stdlib, same trust model, same exit codes as `ito`; it just parses nothing. Since it has no
`--doc` either, read its chapter with `ito --doc irun`.

`-C` applies after every path on the command line, so it moves the work directory but not the ground your paths
stand on: `SCRIPT` and `-r` mean what they mean where you are standing. So `ito ./gen.lua -C build` runs the
`gen.lua` next to you with `fs` resolving relative paths in `build/`, and a script with a `#!/usr/bin/env -S ito -C <dir>` shebang is
still found when executed directly (the kernel hands it over as `./<script>`).

Scripts can read and list files. Writing depends on the program: `iq` cannot write at all, `ied` buffers writes and
applies them only with `-w/--write`, and `ito` writes directly as the script runs — anywhere on the host, not just
under the work directory.

Stdout is populated with the output of the script (`print(...)` function) unless one of those options are used

- just silence script output (`-q/--quiet`)
- output a list of changed files instead (`-l/--list-changed`)
- show a diff of the changes to the files (`-d/--diff`)

The exit code is generally `0` for success and `2` for failure. One can opt into exiting with 1 when changes are
detected (`-e/--exit-code`). There is no built-in reaction to Ctrl-C or any other signal — under `ito`/`irun`,
`proc.trap` is the one place a script opts into handling one; a signal nothing traps keeps its default disposition
(SIGINT and SIGTERM both still terminate the process).

## Scripting

Script help available compiled in: `ito --doc` and `ito --doc <topic>`.

Modules provided

- access the filesystem (`fs`), including an `fd`-like search that can grep the
  files it matches in the same walk (`fs.finder():grep()`, `ito` only)
- manipulate paths (`path`)
- convert between RFC 3339 / RFC 2822 strings, Unix timestamps and duration
  deltas, and render in any IANA time zone (`time`; reading the clock or the
  *host's* zone is `ito` only)
- serialize and deserialize some formats (`json`, `yaml`, `toml`, `hcl`)
- edit structured files in-place, preserving layout and comments (`hcl`, `yaml`)
- render Jinja2 templates (`j2`)
- run local commands (`proc`) and remote ones over SSH (`ssh`), sharing the same
  stream redirect targets (`proc.string()`, `proc.file()`, …), signal a child by
  name or number (`child:signal("TERM")`, `ito` only) and route a stream onto
  this process's own stdout/stderr (`proc.OUT`/`proc.ERR`, the shell's `1>&2`)
- react to a signal sent to *this* process (`proc.trap`, `ito` only), and exit
  immediately with a chosen code (`proc.exit`)
- hash strings, buffers and whole files with MD5/SHA-1/SHA-256/SHA-512 (`hash`;
  the file forms are `ito` only)
- query AWS EC2 and act on instances (`aws.ec2`), read from S3 including object
  versions and delete markers (`aws.s3`), and
  drive Auto Scaling groups (`aws.autoscaling`), against the ambient
  configuration or an explicit region/profile/endpoint (`aws.config`)
- read *and write* tags on all three: any EC2 resource by id
  (`aws.ec2.create_tags`/`delete_tags`), Auto Scaling groups including
  `propagate_at_launch` (`aws.autoscaling.create_or_update_tags`/`delete_tags`),
  and S3 objects and buckets (`aws.s3.get_object_tagging`/`put_object_tagging`/…)
- read environment variables (`os.getenv`, `ito` only) and the running process's
  own pid (`ito.pid`, `ito` only)
- colour terminal output, respecting `NO_COLOR`/`CLICOLOR` and whether stdout is
  a terminal (`("x"):red()`, `ito.colors`, `ito.is_tty`)
- locate the running script's own directory and its import root
  (`ito.require_dir`, `ito.require_root`, `ito` only)
- give a script its own options and arguments, with a generated `--help` (`cli`)
- write raw, unfiltered, ANSI-preserving text to stdout/stderr with or without
  a trailing newline (`io.write`/`io.writeln`/`io.ewrite`/`io.ewriteln`,
  `ito`/`irun` only)
- prompt interactively (`ui.prompt.confirm`/`input`/`password`/`select`/
  `multi_select`, plus a `*_builder()` per kind for the full option set)
  and show progress bars/spinners (`ui.progress.bar`/`spinner`,
  `ui.progress.style()` for custom templates; `ui` is `ito`/`irun` only)
- render a list of rows as an aligned text table, with alignment and border
  presets (`tablefmt`)
- overlap I/O-bound work instead of serializing it, with bounded
  concurrency and results in completion order (`task`)
- guard the running `ito` version (`ito.version_check`)

## Claude / AI skill

Bootstrap the ito skill into Claude once:

```sh
ito --ai-skill-init | claude
```

After each ito update, tell Claude to update the skill:

```
load ito skill. update ito skill
```
