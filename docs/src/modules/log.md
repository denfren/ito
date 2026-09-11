# `log` — logging

| Function | Effect |
| --- | --- |
| `log.error(...)` | log at the `error` level |
| `log.warn(...)` | log at the `warn` level |
| `log.info(...)` | log at the `info` level |
| `log.debug(...)` | log at the `debug` level |
| `log.trace(...)` | log at the `trace` level |

Each function logs its arguments into the host's `tracing` log at the
matching level, under the `ito::lua::log` target. Available in every binary.

## Conventions

**Rendering.** Arguments are joined with a tab, `tostring`-style. Unlike
`print`, a plain table renders as compact JSON (`print` shows
`table: 0x…`); a table with `__tostring` uses it, and a failing `__tostring`
raises. There are no format arguments and no structured fields: build the
message with `string.format` or concatenation.

**Where it goes, and which levels show.** Records go to stderr. The
default level is `info`, so `log.error`/`warn`/`info` show out of the box;
`-v` adds `debug` and `-vv` adds `trace`. Other crates stay at `warn`
whatever `-v` says. `ITO_LOG` (then `RUST_LOG`) replaces that filter
entirely and takes precedence over `-v`; it uses `tracing`'s filter
syntax. An invalid value is reported on stderr and ignored.

```sh
ITO_LOG=ito=debug,warn ito script.luau       # debug for ito, warn for the rest
ITO_LOG=ito::lua::log=trace ito script.luau  # only the script's log.* calls
```

**Separate from `print`.** `print` is never rerouted to the log (under `ied -q` it is silent), and `log.*`
never writes to stdout. `log.*` is unaffected by `-q`, which
silences `print` only. Logging never changes the exit code — `log.error` included; use
`error(...)` to fail the run.

**Styling.** The level label is coloured only when stderr is a terminal and
`NO_COLOR` is unset. ANSI escape codes inside a message are escaped by the
subscriber, not stripped, so styled text shows up as escape sequences in
the log; for styled stderr output use
[`io.stderr:write`](io.md#handlewrite) (not available in `ied`). Records are written without
disturbing live [`ui.progress`](ui-progress.md) bars.

## `log.<level>`

**Signature:** `log.error(...)`, `log.warn(...)`, `log.info(...)`,
`log.debug(...)`, `log.trace(...)`

**Parameters:** any number of values of any type.

**Errors:** raises only on a failing `__tostring`.

```lua
log.info("starting deploy", env)
log.error("failed to write", path, err)
log.debug("state", { hosts = 3 })   -- state	{"hosts":3}
```
