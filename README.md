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

- scripts can include `.rhai` files from the same folder and below
- read and list files (in CWD)
- write and delete files (in CWD, applied after script exits successfully)
  - skip application of changes (`-n/--dry-run`)
  - show a diff (`-d/--diff`)
  - list the changed paths (`-l/--list-changed`)
  - set exit code to 1 when changes are detected (`-e/--exit-code`)
  - suppress script `print()` output (`-q/--quiet`)
  - allow to flush changes during script execution (`--unsafe-fs-flush`)
- manipulate paths (`path` module)
- serialize and deserialize some formats (json, yaml, toml, hcl)
- edit structured files in-place (hcl)
- render Jinja2 templates (`j2` module)
- execute processes (single & batch) gated by the `--unsafe-proc` flag
- guard the running `ito` version (`ito::version_check`)
