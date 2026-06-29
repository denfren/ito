# `io` — raw stream writes

| Function | Signature | Stream | Newline |
| --- | --- | --- | --- |
| `io.write` | `(...)` | stdout | no |
| `io.writeln` | `(...)` | stdout | yes |
| `io.ewrite` | `(...)` | stderr | no |
| `io.ewriteln` | `(...)` | stderr | yes |

`ito`/`irun` only. `iq`'s stdout is its return value and `ied`'s is its
buffered report, so neither can hand a script a second, uncoordinated
writer onto the same stream — `io` is absent there, the same way `fs.write`
is absent under `iq`.

Each function joins its arguments with a tab, `tostring`-style, like
`print`, and writes them straight to the stream with no `tracing`
involved: no level, no target, no `RUST_LOG`/`ITO_LOG` filtering, and no
ANSI stripping. That is the difference from `log.*`, whose output goes
through the host's log and so strips styling codes and can be filtered —
`io.ewriteln` is the way to put styled text on stderr.

```lua
io.writeln(("build ok"):green())     -- stdout, styled, with a newline
io.ewriteln(("warning:"):yellow())   -- stderr, styled, with a newline
io.write("progress: ")               -- stdout, no newline
io.write("50%\n")
```
