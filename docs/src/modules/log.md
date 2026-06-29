# `log` — logging

| Function | Signature | Level |
| --- | --- | --- |
| `log.error` | `(...)` | `error` |
| `log.warn` | `(...)` | `warn` |
| `log.info` | `(...)` | `info` |
| `log.debug` | `(...)` | `debug` |
| `log.trace` | `(...)` | `trace` |

Each function logs its arguments — joined with a tab, `tostring`-style,
like `print` — into the host's `tracing` log at the matching level,
under the `ito::lua::log` target.
Output goes to stderr and is subject to the usual
`tracing`/`RUST_LOG`/`ITO_LOG` filtering and `-v` verbosity flags; it
is independent of `print`'s `--quiet`/`--diff`/`--list-changed`
suppression — `log.*` always logs. Going through `tracing` also means
ANSI styling codes are stripped; for styled or otherwise raw stderr
output, use `io.ewrite`/`io.ewriteln` (`ito`/`irun` only) instead.

```lua
log.info("starting deploy", env)
log.error("failed to write", path, err)
```
