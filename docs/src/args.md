# `args` — injected globals

Values passed on the command line are injected as plain **globals** into
the script's scope (not a module namespace). Each flag may be repeated.

| Flag | Long | Form | Notes |
| --- | --- | --- | --- |
| `-S` | `--string <name> <value>` | string | |
| `-N` | `--numeric <name> <value>` | int or float | Parsed as `i64`, else `f64`. |
| `-B` | `--bool <name> <value>`    | bool | `true` / `false`. |

The injected `<name>`s become ordinary globals you can reference directly,
but only in the **entry** script (they are not seeded into imported
modules).

Two host-provided constants are pre-bound and *are* visible everywhere —
in the entry script and inside imported modules and their functions:

- `MAIN` — `true` in the entry script, `false` inside an imported module
  (a module is never the entry point), so a script can guard
  entry-point-only logic.
- `ITO_VERSION` — the running `ito` version as a string (e.g. `"0.4.1"`),
  matching `ito --version`.

```sh
ito exec '
  print(`hello ${name}`);   // "hello world"
  if verbose { print(count + 1); }   // 4
' -S name world -N count 3 -B verbose true
```

```rhai
// guard entry-point-only logic
if MAIN {
    print("running as main");
}
```
