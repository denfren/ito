# args — passing values to scripts

## Script flags (preferred)

Pass named options and boolean flags directly on the command line. They
are stripped before `ito`'s own flag parsing, so they can appear
anywhere on the command line.

| Form | Example | Access in script |
| --- | --- | --- |
| `key=value` | `env=prod` | `ito::opt("env")` → `"prod"` |
| `+flag` | `+dry-run` | `ito::flag("dry-run")` → `true` |

```sh
ito run deploy.rhai env=prod +dry-run
ito deploy env=prod +dry-run
```

See [`ito` module](modules/ito.md) for `ito::flag`, `ito::opt`, and
`ito::try_opt`.

## Injected globals (`-S`/`-N`/`-B`) *(deprecated)*

> **Deprecated.** Prefer `key=value` / `+flag` and the `ito::opt` /
> `ito::flag` functions. The `-S`/`-N`/`-B` flags will be removed in a
> future release.

Values can also be injected as typed **globals** directly into the
script's scope. Each flag may be repeated.

| Flag | Long | Type | Notes |
| --- | --- | --- | --- |
| `-S` | `--string <name> <value>` | string | |
| `-N` | `--numeric <name> <value>` | int or float | Parsed as `i64`, else `f64`. |
| `-B` | `--bool <name> <value>` | bool | `true` / `false`. |

The injected `<name>`s become ordinary globals accessible directly, but
only in the **entry** script (they are not seeded into imported modules).

```sh
ito exec '
  print(`hello ${name}`);   // "hello world"
  if verbose { print(count + 1); }   // 4
' -S name world -N count 3 -B verbose true
```

## Host-provided constants

Two constants are pre-bound and visible everywhere — in the entry script
and inside all imported modules:

- `MAIN` — `true` in the entry script, `false` inside an imported module.
- `ITO_VERSION` — the running `ito` version as a string (e.g. `"0.4.0"`).

```rhai
if MAIN {
    print("running as main");
}
```
