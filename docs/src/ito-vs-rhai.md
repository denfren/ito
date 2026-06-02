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

## Base-type patches

ito patches Rhai's built-in **string** and **array** types directly —
no import needed, they work as plain method calls on any string or array
value.

### String — patched (return-value fix)

Rhai's built-in string mutators return unit. ito overrides them so they
**also return the resulting string**, enabling assignment and chaining:

| Method | Change |
| --- | --- |
| `s.trim()` | Now returns `s` after mutating. |
| `s.make_upper()` | Now returns `s` after mutating. |
| `s.make_lower()` | Now returns `s` after mutating. |
| `s.clear()` | Now returns `s` after mutating. |
| `s.truncate(len)` | Now returns `s` after mutating. |
| `s.crop(start)` | Now returns `s` after mutating. |
| `s.crop(start, len)` | Now returns `s` after mutating. |
| `s.set(index, ch)` | Now returns `s` after mutating. |
| `s.pad(len, fill)` | Now returns `s` after mutating. |
| `s.remove(sub)` | Now returns `s` after mutating. |
| `s.replace(from, to)` | Now returns `s` after mutating. |

### String — added

| Method | What it does |
| --- | --- |
| `s.trim_start()` | Strip leading whitespace; return result. |
| `s.trim_end()` | Strip trailing whitespace; return result. |

### Array — added

| Method | What it does |
| --- | --- |
| `arr.join(sep)` | Concatenate string elements separated by `<sep>`; return result. |
