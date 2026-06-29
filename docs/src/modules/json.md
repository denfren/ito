# `json` — JSON

| Function | Effect |
| --- | --- |
| `json.parse(text)` | Parse JSON text into native Luau values. |
| `json.to_string(value)` | Serialize a Luau value to compact JSON text. |
| `json.to_string_pretty(value)` | Serialize a Luau value to 2-space-indented JSON text. |
| `json.null` | The null sentinel — identical to `ito.null`. |

```lua
local v = json.parse(fs.read("package.json"))
print(v.name)
for dep in pairs(v.dependencies) do print(dep) end
```

## Value mapping

`parse` converts a `serde_json::Value` to Luau: objects become keyed
tables, arrays become sequential tables, strings/numbers/booleans map to
their Luau equivalents, and `null` becomes the `json.null` sentinel (see
below) rather than Luau `nil`. `to_string`/`to_string_pretty` convert the
other way, so a value built entirely from tables, strings, numbers,
booleans and `json.null` round-trips.

### `null`

A Luau table cannot store `nil` as a value — assigning `t.k = nil`
deletes the key — so a JSON `null` cannot become plain Luau `nil`
without losing the fact that the key was present at all. `parse`
instead produces a shared sentinel value, spelled either `json.null` or
[`ito.null`](ito.md) — the *same* value (`json.null == ito.null`), not
two that merely compare equal, so either name works wherever the other
does. `v.a == nil` is `false` for a key whose JSON value was `null`, but
`ito.is_null(v.a)` is `true`. The sentinel works in both directions —
write it into a table you build yourself and `to_string` emits a real
JSON `null`:

```lua
local v = json.parse('{"a":null,"b":1}')
ito.is_null(v.a)              -- true
json.to_string(v)             -- '{"a":null,"b":1}'  (round-trips)

json.to_string({a = json.null, b = 1})  -- '{"a":null,"b":1}'
json.null == ito.null                   -- true (identical value)
```

An **omitted** key, as opposed to one explicitly set to `json.null`,
simply does not appear in the output — a Luau table cannot "contain" a
`nil` value at all:

```lua
json.to_string({a = 1})   -- '{"a":1}'  (no "b" key at all)
```

## `json.parse`

Deserialize JSON text into native Luau values.

**Signature:** `json.parse(text: string) -> any`

**Parameters:**

- `text` — the JSON document to parse.

**Returns:** the parsed value — a table (object or array), string,
number, boolean, or `json.null`.

**Errors:** raises if `text` is not valid JSON (`"json parse error: ..."`,
from serde_json).

```lua
local v = json.parse('{"a":[1,2,3],"b":"x"}')
print(v.a[2])   -- 2
print(v.b)      -- x
```

## `json.to_string`

Serialize a Luau value to compact JSON text.

**Signature:** `json.to_string(value: any) -> string`

**Parameters:**

- `value` — a table, string, number, boolean, or `json.null` (nested
  arbitrarily).

**Returns:** the compact JSON encoding (no extra whitespace).

**Errors:** raises if `value` contains something that cannot be
represented as JSON (`"json serialize error: ..."`) — e.g. a function,
thread, or userdata.

```lua
json.to_string({a = 1, b = {2, 3}})  -- '{"a":1,"b":[2,3]}'
```

## `json.to_string_pretty`

Serialize a Luau value to JSON text indented with 2 spaces per level.

**Signature:** `json.to_string_pretty(value: any) -> string`

**Parameters:**

- `value` — same as `to_string`.

**Returns:** the pretty-printed JSON encoding.

**Errors:** same as `to_string`.

```lua
print(json.to_string_pretty({a = 1, b = {2, 3}}))
-- {
--   "a": 1,
--   "b": [
--     2,
--     3
--   ]
-- }
```
