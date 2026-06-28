# ito - infrastructure tool

A [Rhai](https://rhai.rs) script runner with additional modules for IaC repositories.

## Quickstart

**install from source**

```sh
cargo install --path ito
```

Try a one-liner. `ito exec` runs a script you type right on the command
line. Here it reads this project's `Cargo.toml` and prints the version:

```sh
# The code can only access files in $PWD, for the script our $PWD is the root folder
ito exec 'print(toml::parse(fs::read("/Cargo.toml")).package.version)'
```

## How it works

`ito` restricts the script file access to the current work directory which can be changed with `-C/--directory <path>`
(like in `make`). You provide a script file that gets executed. The script file can include other script files from its
own directory or subdirectories thereof. Note that when invoking with a script on the command line
(`ito exec <script>`) or with stdin as the script (`ito run -`) then the imports are rooted at the working directory.

Scripts can read, list, write and delete files in the current work directory unless `-n/--dry-run` is given.

Stdout is populated with the output of the script (`print(...)` function) unless one of those options are used

- just silence script output (`-q/--quiet`)
- output a list of changed files instead (`-l/--list-changed`)
- show a diff of the changes to the files (`-d/--diff`)

The exit code is generally `0` for success and `2` for failure. One can opt into exiting with 1 when changes are
detected (`-e/--exit-code`).

## Scripting

Script help available compiled in: `ito help` and `ito help <topic>`.

Modules provided

- access filesystemd (`fs`)
- manipulate paths (`path`)
- serialize and deserialize some formats (`json`, `yaml`, `toml`, `hcl`)
- edit structured files in-place, preserving layout and comments (`hcl`, `yaml`)
- render Jinja2 templates (`j2`)
- guard the running `ito` version (`ito::version_check`)

Experimental features gated behind flags:

- allow to flush file changes during script execution (`--unsafe-fs-flush`)
- allow execution of processes (single & batch) (`--unsafe-proc`)
