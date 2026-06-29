# `toml` — TOML

| Function | Effect |
| --- | --- |
| `toml.parse(text)` | Parse TOML text into native Luau values. |
| `toml.to_string(value)` | Serialize a Luau value to TOML text. |
| `toml.to_string_pretty(value)` | Serialize a Luau value to pretty-printed TOML text. |

```lua
local cfg = toml.parse(fs.read("Cargo.toml"))
print(cfg.package.version)
```

`parse` decodes via the `toml` crate into a `toml::Value`, then converts
to Luau: tables (keyed for a TOML table, sequential for an array),
strings, numbers, and booleans map to their Luau equivalents; datetimes
become strings (TOML's native datetime type has no Luau counterpart).
`to_string`/`to_string_pretty` convert the other way. This is a plain
parse/serialize, not a format-preserving editor (unlike `yaml.edit`):
round-tripping a file through `parse` and `to_string` regenerates
layout, comments and formatting rather than preserving the original.

The top-level value passed to `to_string`/`to_string_pretty` must be a
table with string keys — TOML has no top-level array or scalar form.

TOML has no `null` value. Writing [`ito.null`](ito.md) — or any other
sentinel that isn't a plain Luau value — into a table passed to
`to_string`/`to_string_pretty` is a runtime error, since there is no
TOML value it can become.

## `toml.parse`

Deserialize TOML text into native Luau values.

**Signature:** `toml.parse(text: string) -> any`

**Parameters:**

- `text` — the TOML document to parse.

**Returns:** the parsed value — normally a table, since a valid TOML
document is always a table at the top level.

**Errors:** raises if `text` is not valid TOML (`"toml parse error: ..."`,
from the `toml` crate).

```lua
local cfg = toml.parse([[
  [package]
  name = "ito"
  version = "0.6.0"
]])
print(cfg.package.name)   -- ito
```

## `toml.to_string`

Serialize a Luau value to TOML text.

**Signature:** `toml.to_string(value: any) -> string`

**Parameters:**

- `value` — a table with string keys at the top level (nested tables,
  arrays, strings, numbers and booleans are all fine inside it).

**Returns:** the TOML encoding.

**Errors:** raises (`"toml serialize error: ..."`) if `value` isn't a
table with string keys at the top level, or contains something with no
TOML representation — e.g. a function, thread, userdata, or `ito.null`.

```lua
toml.to_string({package = {name = "ito", version = "0.6.0"}})
-- [package]
-- name = "ito"
-- version = "0.6.0"
```

## `toml.to_string_pretty`

Serialize a Luau value to pretty-printed TOML text.

**Signature:** `toml.to_string_pretty(value: any) -> string`

**Parameters:**

- `value` — same as `to_string`.

**Returns:** the pretty-printed TOML encoding (e.g. arrays spread across
multiple lines).

**Errors:** same as `to_string`.

```lua
print(toml.to_string_pretty({ports = {80, 443, 8080}}))
-- ports = [
--     80,
--     443,
--     8080,
-- ]
```
