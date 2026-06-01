# `ito` — runtime guard

| Function | Effect |
| --- | --- |
| `ito::version_check(req)` | Abort if the running `ito` version is not semver-compatible with `req`. |

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
ito::version_check("0.1.0");
```

The injected `ITO_VERSION` global holds the running version as a string
if you want to branch on it instead of aborting.
