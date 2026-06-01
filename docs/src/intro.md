# ito

A [Rhai](https://rhai.rs) script runner with a small standard library for
inspecting and generating config files. Scripts get a virtual filesystem
(`fs`) rooted at the working directory, plus `path`, `json`, `toml`,
`yaml`, `hcl`, and `j2` modules for path manipulation, parsing, emitting
structured data, and rendering templates, an opt-in `proc` module for
running processes, and an `ito` module for guarding the running version.

There is no positional input — the only relevant path is the working
directory, which becomes the root of the `fs` module (override with `-C`).
File I/O happens through `fs`; buffered writes are flushed to disk after
the script returns. Exit codes: `0` success, `1` success with changes
(only under `-e`/`--exit-code`), `2` a generic failure (usage,
environment, output), and `3` a failure in the script itself (parse or
runtime error).

## Help from the CLI

This documentation is embedded in the `ito` binary. List the available
help topics and print any of them:

```
ito help        # list the available topics
ito help fs     # print one topic
ito help all    # the full reference in one output
```

`ito --help` / `ito -h` cover the command-line flags themselves.
