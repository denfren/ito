# ito vs vanilla Rhai

ito embeds the [Rhai scripting language](https://rhai.rs/book/) and
extends it in two ways: **modules** (namespaced functions for
structured formats, filesystem, processes, …) and **base-type patches**
(overrides and additions on Rhai's built-in string and array types).

## Modules provided by ito

These are not in vanilla Rhai — import or call them by their `ns::`
prefix:

| Module | What it provides |
| --- | --- |
| [`string`](modules/string.md) | String method patches (see below) |
| [`array`](modules/array.md) | Array method patches (see below) |
| [`map`](modules/map.md) | Object-map reference |
| [`fs`](modules/fs.md) | Virtual filesystem — read, write, list, remove |
| [`path`](modules/path.md) | Path manipulation |
| [`re`](modules/re.md) | Regular expressions |
| [`json`](modules/json.md) | JSON parse / emit |
| [`toml`](modules/toml.md) | TOML parse / emit |
| [`yaml`](modules/yaml.md) | YAML parse / emit |
| [`hcl`](modules/hcl.md) | HCL parse / emit |
| [`j2`](modules/j2.md) | Jinja2 template rendering |
| [`proc`](modules/proc.md) | Process execution (`--unsafe-proc` required) |
| [`ito`](modules/ito.md) | Runtime guards (`ito::assert`, …) |

## Base-type additions

ito adds methods to Rhai's built-in **string** and **array** types
directly — no import needed, they work as plain method calls on any
string or array value.

### String — added

ito does not override Rhai's built-in string methods. Instead it adds
explicit `to_*` (pure, returns a new string) and `make_*` (mutates in
place, returns the result) variants for each operation. See
[`string`](modules/string.md) for the full list.

Summary:

| Operation | Pure (`to_*`) | Mutating (`make_*`) |
| --- | --- | --- |
| Trim | `to_trimmed()`, `to_trimmed_start()`, `to_trimmed_end()` | `make_trimmed()`, … |
| Case | — (Rhai has `to_upper`/`to_lower`) | `make_upper()`, `make_lower()` |
| Clear | `to_cleared()` | `make_cleared()` |
| Truncate | `to_truncated(len)` | `make_truncated(len)` |
| Crop | `to_cropped(start[, len])` | `make_cropped(start[, len])` |
| Set char | `to_set(idx, ch)` | `make_set(idx, ch)` |
| Pad | `to_padded(len, fill)` | `make_padded(len, fill)` |
| Remove | `to_removed(sub)` | `make_removed(sub)` |
| Replace | `to_replaced(from, to)` | `make_replaced(from, to)` |

### Array — added

| Method | What it does |
| --- | --- |
| `arr.join(sep)` | Concatenate string elements separated by `<sep>`; return result. |
