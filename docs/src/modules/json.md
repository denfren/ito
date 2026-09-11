# `json` — JSON

| Function | Effect |
| --- | --- |
| `json.parse(text, [name])` | Parse JSON text into native Luau values. |
| `json.to_string(value, [opts])` | Serialize a Luau value to JSON text (compact, or pretty with `opts`). |
| `json.parse_lines(text, [name])` | Parse JSON Lines into an array of values. |
| `json.to_string_lines(array)` | Serialize an array of values as JSON Lines. |

The null sentinel is the global [`null`](table.md#the-global-null); the array marker is
`table.array` (see below).

## Example

```lua
local v = json.parse(fs.read("package.json"))
print(v.name)
for dep in pairs(v.dependencies) do print(dep) end
```

## Value mapping

`parse` converts objects to keyed tables, arrays to sequential tables,
strings/numbers/booleans to their Luau equivalents, and `null` to the global
`null` sentinel rather than Luau `nil`. `to_string` converts the other way,
so a value built from tables, strings, numbers, booleans and `null`
round-trips.

Lossy and surprising cases:

- Luau numbers are doubles: integers above 2^53 lose precision when parsed,
  and `1.0` is written as `1`.
- Object keys are written **sorted**, not in source order.
- An empty Luau table is an object (`{}`). To write `[]`, mark the table with
  `table.array()` (arrays produced by `parse` already carry the marker, so
  `[]` round-trips).
- A table is an array only if its keys are exactly `1..n`. A table mixing
  array items and string keys, or with sparse integer keys, raises.
- NaN and infinity raise. Strings must be valid UTF-8; invalid bytes raise.
  Functions, threads and userdata raise, as does a cyclic table.

Serialization errors name the offending path, for example
``json serialize error at `a.b[2]`: ...``.

### `null`

A Luau table cannot store `nil` as a value — `t.k = nil` deletes the key —
so `parse` produces the shared `null` sentinel. `v.a == nil` is `false` for
a key whose JSON value was `null`; `v.a == null` is `true`. The sentinel
works in both directions:

```lua
local v = json.parse('{"a":null,"b":1}')
v.a == null                          -- true
json.to_string(v)                    -- '{"a":null,"b":1}'
json.to_string({a = null, b = 1})    -- '{"a":null,"b":1}'
json.to_string({a = 1})              -- '{"a":1}' (an omitted key stays omitted)
```

### The array marker

```lua
json.to_string({items = {}})                 -- '{"items":{}}'
json.to_string({items = table.array()})      -- '{"items":[]}'
json.to_string({items = table.array({1,2})}) -- '{"items":[1,2]}'
```

`table.array([t])` marks `t` (or a fresh empty table) as an array and
returns it. The marker is shared by `json`, `toml`, `yaml` and `hcl`.

## `json.parse`

Deserialize JSON text into native Luau values.

**Signature:** `json.parse(text: string, name: string?) -> any`

**Parameters:**

- `text` — the JSON document.
- `name` — optional, added to parse errors (typically the file name).

**Returns:** a table (object or array), string, number, boolean, or `null`.

**Errors:** raises on invalid JSON: `json parse error in <name>: ...` (or
without `in <name>` when no name was given).

```lua
local v = json.parse('{"a":[1,2,3],"b":"x"}')
print(v.a[2])   -- 2
json.parse(fs.read("a.json"), "a.json")
```

## `json.to_string`

Serialize a Luau value to JSON text.

**Signature:** `json.to_string(value: any, opts: table?) -> string`

**Parameters:**

- `value` — a table, string, number, boolean, or `null`, nested arbitrarily.
- `opts.pretty` — `true` for multi-line output (2-space indent, ending with
  a newline).
- `opts.indent` — integer >= 0, the indent width; implies `pretty`.
  Unknown option keys raise.

**Returns:** the JSON text. Compact output has no whitespace and no trailing
newline.

**Errors:** raises for the unrepresentable values listed under
[Value mapping](#value-mapping).

```lua
json.to_string({a = 1, b = {2, 3}})            -- '{"a":1,"b":[2,3]}'
print(json.to_string({a = 1, b = {2, 3}}, {pretty = true}))
-- {
--   "a": 1,
--   "b": [
--     2,
--     3
--   ]
-- }
json.to_string({a = 1}, {indent = 4})          -- "{\n    \"a\": 1\n}\n"
```

## `json.parse_lines`

Parse JSON Lines: one JSON value per line. Blank lines are skipped.

**Signature:** `json.parse_lines(text: string, name: string?) -> array`

**Errors:** raises on an invalid line, naming the line number.

```lua
local rows = json.parse_lines('{"a":1}\n{"a":2}\n')
print(#rows, rows[2].a)   -- 2  2
```

## `json.to_string_lines`

Serialize each element of an array as one compact line.

**Signature:** `json.to_string_lines(values: array) -> string`

**Returns:** the lines, each ending in a newline.

**Errors:** as `to_string`, per element.

```lua
json.to_string_lines({1, {a = 2}})   -- "1\n{\"a\":2}\n"
```
