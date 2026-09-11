# `toml` — TOML

| Function | Effect |
| --- | --- |
| `toml.parse(text, [name])` | Parse TOML text into native Luau values. |
| `toml.to_string(value, [opts])` | Serialize a Luau value to TOML text (`{pretty = true}` for the pretty layout). |

## Example

```lua
local cfg = toml.parse(fs.read("Cargo.toml"))
print(cfg.package.version)
```

## Value mapping

`parse` converts a TOML table to a keyed table, an array to a sequential
table, and strings, numbers and booleans to their Luau equivalents.
Datetimes become **strings** holding their TOML text (`"1979-05-27T07:32:00Z"`);
there is no way to write one back — a string stays a string.

This is a plain parse/serialize, not a format-preserving editor (there is no
`toml.edit`): round-tripping a file regenerates layout, comments and
formatting, and writes keys **sorted**.

- Luau numbers are doubles: integers above 2^53 lose precision, and `1.0`
  writes back as the integer `1`.
- TOML has **no null**: writing [`null`](table.md#the-global-null) raises, naming the key path.
  `parse` therefore never produces `null`.
- An empty Luau table is a table (`[t]`); mark an empty array with
  `table.array()` (see [`json`](json.md)). Arrays from `parse` carry the
  marker, so `a = []` round-trips.
- The top-level value must be a table with string keys.
- Mixed or sparse tables, NaN-free but unrepresentable values (functions,
  userdata), invalid UTF-8 and cycles raise with the offending path.

## `toml.parse`

Deserialize TOML text into native Luau values.

**Signature:** `toml.parse(text: string, name: string?) -> any`

**Parameters:**

- `text` — the TOML document.
- `name` — optional, added to parse errors (typically the file name).

**Returns:** a table (a TOML document is always a table at the top level).

**Errors:** raises on invalid TOML with the `toml` crate's message
(`TOML parse error at line ...`), prefixed by `in <name>: ` when a name was
given.

```lua
local cfg = toml.parse([[
  [package]
  name = "ito"
  version = "0.20.3"
]])
print(cfg.package.name)   -- ito
```

## `toml.to_string`

Serialize a Luau value to TOML text.

**Signature:** `toml.to_string(value: table, opts: table?) -> string`

**Parameters:**

- `value` — a table with string keys at the top level.
- `opts.pretty` — `true` for the pretty layout (arrays spread over several
  lines). There is no `indent` option for TOML; unknown keys raise.

**Returns:** the TOML text.

**Errors:** raises (`toml serialize error ...`) if `value` is not a table
with string keys, or contains something with no TOML form (see above).

```lua
toml.to_string({package = {name = "ito", version = "0.20.3"}})
-- [package]
-- name = "ito"
-- version = "0.20.3"

print(toml.to_string({ports = {80, 443, 8080}}, {pretty = true}))
-- ports = [
--     80,
--     443,
--     8080,
-- ]
```
