# `ito` — runtime utilities

## Version guard

| Function | Signature | Effect |
| --- | --- | --- |
| `ito::version_check` | `(req: String)` | Abort if the running `ito` is not semver-compatible with `req`. |

`version_check` lets a script pin the `ito` version it was authored
against — a lightweight, in-script "lock file". Call it near the top of
a script so an incompatible `ito` fails fast with a descriptive error
(exit `3`) instead of misbehaving later.

`req` is normally a bare version literal (`<major>.<minor>.<patch>`,
read with caret/`^` semantics): the leftmost non-zero component is the
breaking axis, and newer versions within it are accepted. So
`"0.2.1"` accepts `>=0.2.1, <0.3.0`, and `"1.2.3"` accepts
`>=1.2.3, <2.0.0`. Any string a semver `VersionReq` accepts also works
(e.g. `">=0.2, <0.4"`, `"^0.2.1"`).

```rhai
ito::version_check("0.4.0");
```

The injected `ITO_VERSION` global holds the running version as a string
if you want to branch on it instead of aborting.

## Script flags

Script flags are passed on the command line before any `ito` flags:

- `key=value` — a named option (string value).
- `+flag` — a boolean flag (present = true, absent = false).

```sh
ito run deploy.rhai env=prod +dry-run
ito deploy env=prod +dry-run
```

| Function | Signature | Returns |
| --- | --- | --- |
| `ito::flag` | `(name: String) -> bool` | `true` when `+name` was passed, `false` otherwise. |
| `ito::opt` | `(name: String) -> String` | Value of `name=value`; throws (exit `3`) when absent. |
| `ito::try_opt` | `(name: String) -> String \| ()` | Value of `name=value`, or unit `()` when absent. |

```rhai
let env = ito::opt("env");          // "prod" — throws if not passed

let tag = ito::try_opt("tag");      // () when not passed
if tag != () {
    print(`deploying tag ${tag}`);
}

if ito::flag("dry-run") {
    print("dry run — no changes written");
}
```
