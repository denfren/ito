# ito - infrastructure tool

A [Luau](https://luau.org) script runner with additional modules for IaC repositories.

## Quickstart

**install from source**

```sh
cargo install --path ito
```

Try a small script. This one reads this project's `Cargo.toml` and prints the version:

```sh
echo 'print(toml.parse(fs.read("ito/Cargo.toml"))["package"].version)' > version.luau
# Relative paths resolve against $PWD, which `-C` moves
ito version.luau
```

## How it works

Relative paths in ito's own stdlib (`fs`, `j2`, …) resolve against the current work directory, which can be
changed with `-C/--directory <path>` (like in `make`). It is only a starting point:
`..` and absolute paths reach out. Traversals report paths relative to that directory; only a path that
genuinely lies outside it comes back absolute.

You provide a script file that gets executed. The script file can split itself across other `.lua`/`.luau` files with plain Luau `require`, resolved relative to its own directory
and **confined to that tree** — a `require("../../anything.lua")` that climbs out is an error, not a read. An
`@alias/path` import resolves through the `aliases` table of the nearest `.luaurc`, and is confined the same way.
Every program reads `<program> [<flags>] <script> [<args>...]`: its own flags go before the script path, and
everything after it belongs to the script, arriving verbatim as the global `argv` (`ito -v deploy.luau --env prod`)
— flag-looking tokens included, so a script's flags can never collide with the program's. Nothing is interpreted
— no option map, no types — so a script parses its arguments however it likes (or declares them with `cli`).
`argv[1]` is the first argument given, not the program name, and a run without arguments sees an empty table. A
`#!/usr/bin/env -S ito` script therefore owns its whole command line: `./deploy.luau --env prod -C x` passes all
four strings through untouched. The shebang line is kept as a Luau comment, so line numbers in errors still match
the file.

`-C` applies after every path on the command line, so it moves the work directory but not the ground your paths
stand on: `<script>` means what it means where you are standing. So `ito -C build ./gen.lua` runs the
`gen.lua` next to you with `fs` resolving relative paths in `build/`, and a script with a `#!/usr/bin/env -S ito -C <dir>` shebang is
still found when executed directly (the kernel hands it over as `./<script>`).

Writes land directly as the script runs — anywhere on the host, not just under the work directory.

Stdout carries what the script `print`s (plain Luau `print`: `tostring` of each argument, tab-joined, newline).
A script controls its own output; `-v`/`-vv` only raise log verbosity.

The exit code is `0` for success (`1` under `ied -e` when the run changed something), `2` for a failure that is
not the script's fault, `3` for one that is, and `141` when the output pipe went away (`| head`; 128 + SIGPIPE).
There is no built-in reaction to Ctrl-C or any other signal — `proc.trap` is the one place a script opts into handling
one; a signal nothing traps keeps its default disposition (SIGINT and SIGTERM both still terminate the process).

## ied

`ied` is a restricted variant of `ito` for editing files. Its writes are buffered and applied only with
`-w/--write`, so a bare run computes the changes, reports on them and drops them (which also makes it the
read-only query tool):

- `-d/--diff` shows the changes as a diff
- `-l/--list-changed` lists the changed files
- `-e/--exit-code` exits 1 when there are changes
- `-q/--quiet` silences the script's `print` (`-d` and `-l` imply it, since the report owns stdout)

An `ied` run is reproducible: the same work-directory state and the same script always give the same result. So
`fs` is rooted at the work directory (a path escaping it is an error), and everything else that could
change the outcome is absent: the clock and host time zone, the environment, randomness, `proc`/`ssh`/`aws`/
`task`, and the terminal (`io`, `ui`). Everything not absent works as in `ito`; see
[Reproducibility](docs/src/binaries/ied.md#reproducibility) for the full list.

## Scripting

Script help available compiled in: `ito --doc` and `ito --doc <topic>`.

Modules provided

- access the filesystem (`fs`), including an `fd`-like search that can grep the
  files it matches in the same walk (`fs.finder():grep()`)
- manipulate paths (`path`)
- match, capture, replace and split with RE2/Perl-style regular expressions
  (`re`; not Lua patterns)
- convert between RFC 3339 / RFC 2822 strings, Unix timestamps and duration
  deltas, and render in any IANA time zone (`time`; reading the clock or the
  *host's* zone is absent under `ied`)
- serialize and deserialize some formats (`json`, `yaml`, `toml`, `hcl`)
- edit structured files in-place, preserving layout and comments (`hcl`, `yaml`)
- render Jinja2 templates (`j2`)
- run local commands (`proc`) and remote ones over SSH (`ssh`), sharing the same
  stream redirect targets (`proc.string()`, `proc.file()`, …), signal a child by
  name or number (`child:signal("TERM")`) and route a stream onto
  this process's own stdout/stderr (`proc.OUT`/`proc.ERR`, the shell's `1>&2`)
- react to a signal sent to *this* process (`proc.trap`), and exit
  immediately with a chosen code (`proc.exit`)
- hash strings, buffers and whole files with MD5/SHA-1/SHA-224/SHA-256/SHA-384/
  SHA-512 (`hash`; the file forms are absent under `ied`)
- encode and decode base64, with an engine builder for other alphabets and
  padding (`encoding`)
- encrypt and decrypt Ansible Vault 1.1 text (`ansible.vault.encrypt`/
  `decrypt`/`is_vault`), and load a vault password the way `ansible-vault
  --vault-password-file` does (`ansible.vault.load_password_file`; `encrypt`
  and `load_password_file` are absent under `ied`)
- query AWS EC2 and act on instances (`aws.ec2`), read from S3 including object
  versions and delete markers (`aws.s3`),
  drive Auto Scaling groups (`aws.autoscaling`), look up CloudTrail's API
  call history (`aws.cloudtrail.lookup_events`), and read CloudWatch metrics
  and the dimension sets they are published under
  (`aws.cloudwatch.get_metric_statistics`/`list_metrics`), against the ambient
  configuration or an explicit region/profile/endpoint (`aws.config`)
- read *and write* tags on: any EC2 resource by id
  (`aws.ec2.create_tags`/`delete_tags`), Auto Scaling groups including
  `propagate_at_launch` (`aws.autoscaling.create_or_update_tags`/`delete_tags`),
  and S3 objects and buckets (`aws.s3.get_object_tagging`/`put_object_tagging`/…)
- read environment variables (`os.getenv`) and the running process's
  own pid (`proc.pid`)
- colour terminal output, respecting `NO_COLOR`/`CLICOLOR` and whether stdout is
  a terminal (`("x"):red()`, `fmt.colors`; `fmt.is_tty` is absent under `ied`), plus
  text helpers, status symbols and aligned tables (`fmt`); the text helpers are
  also methods on Luau strings (`s:trimmed()`, `s:red()`)
- locate the running script, its own directory and its import root
  (`fs.script`, `fs.require_dir`, `fs.require_root`)
- a global `null` for JSON/YAML/HCL nulls, and `table.array(t)` to mark a table
  as an array (so an empty one serializes as `[]`)
- give a script its own options and arguments, with a generated `--help` (`cli`)
- write raw, unfiltered, ANSI-preserving text to stdout/stderr and read stdin
  through file-like handles (`io.stdout:write(...)`, `io.stderr:write(...)`,
  `io.stdin:read(...)`/`:lines()`, plus `io.write`/`io.read`/`io.lines`/
  `io.flush` on the defaults; absent under `ied`, which has `print` alone)
- log to stderr at `error`/`warn`/`info`/`debug`/`trace`, separate from the
  script's `print` output (`log`; `-v`/`-vv` and `ITO_LOG` control which levels
  show)
- prompt interactively (`ui.prompt.confirm`/`input`/`password`/`select`/
  `multi_select`, plus a `*_builder()` per kind for the full option set)
  and show progress bars/spinners (`ui.progress.bar`/`spinner`,
  `ui.progress.style()` for custom templates; `ui` is absent under `ied`)
- render a list of rows as an aligned text table, with alignment and border
  presets (`fmt.table.new`)
- overlap I/O-bound work instead of serializing it, with bounded
  concurrency and results in completion order (`task`)
- guard the running `ito` version (`ito.version_check`) and make a random
  adjective-noun name (`ito.slug`; absent under `ied`)

## Claude / AI skill

Bootstrap the ito skill into Claude once:

```sh
ito --ai-skill-init | claude
```

After each ito update, tell Claude to update the skill:

```
load ito skill. update ito skill
```
