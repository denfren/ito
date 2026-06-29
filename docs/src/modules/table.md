# `table` — arrays and objects

Luau has one built-in `table` type for both arrays and objects: a table
used with sequential integer keys starting at `1` is an array; a table
used with string (or other) keys is a map. This chapter documents Luau's
built-in `table` type and library as used in ito scripts — ito adds no
functions of its own here. Full built-in reference:
[luau-lang.org — Tables](https://luau.org/syntax#table-constructors) and
[Luau's `table` library](https://luau.org/library#table-library).

| Function | Effect |
| --- | --- |
| `#xs` | Element count of the array part. |
| `table.insert(xs, val)` | Append `val` to the end. |
| `table.insert(xs, i, val)` | Insert `val` before index `i`. |
| `table.remove(xs)` | Remove and return the last element. |
| `table.remove(xs, i)` | Remove and return the element at index `i`. |
| `table.find(xs, val)` | Index of the first `val`, or `nil`. |
| `table.sort(xs[, cmp])` | Sort the array part in place. |
| `table.concat(xs[, sep])` | Join stringable elements into one string. |
| `table.clone(t)` | Shallow copy. |

```lua
-- Parsed config comes back as tables; walk it with pairs/ipairs.
local v = json.parse(fs.read("package.json"))
for dep, version in pairs(v.dependencies) do
    print(dep .. " = " .. version)
end

local scripts = {}
for name in pairs(v.scripts) do
    table.insert(scripts, name)
end
table.sort(scripts)
-- scripts is now an alphabetically sorted array of script names
```

## General rules

- A table can mix both parts: `{1, 2, extra = "x"}` has a 2-element
  array part and an `extra` key. `#t` only counts the array part, and
  its result is otherwise undefined if the array part has holes (a
  `nil` between two non-`nil` indices).
- Reading a missing key returns `nil`, never an error; setting a key to
  `nil` removes it.
- `pairs()` iteration order over non-array (map) keys is **unspecified**
  — do not rely on it. If a specific emission order matters (e.g.
  building a document with `yaml.edit`/`hcl.edit`), add entries one at a
  time with their structural-edit methods (`append`, `add`, …) instead
  of constructing one large table literal.
- Luau has no `map`/`filter`/`reduce`/`for_each` on arrays — write a
  plain `for` loop.
- There is no built-in `keys()`/`values()`/`has_key()` helper; build one
  with `pairs` if needed.

## `#xs` (length operator)

Element count of a table's array part.

**Signature:** `#xs -> number`

**Parameters:**
- `xs` — table to measure.

**Return value:** the number of elements in the array part (indices `1`
through the first `nil`). Behavior is undefined if the array part has
holes.

**Error conditions:** none for a table; raises if applied to a value
that has no length (e.g. `nil`, a number).

```lua
local xs = {1, 2, 3}
#xs   -- 3
```

## `table.insert`

Appends a value to an array, or inserts it at a given index, shifting
later elements up.

**Signature:**
`table.insert(xs: table, val: any) -> ()`
`table.insert(xs: table, i: number, val: any) -> ()`

**Parameters:**
- `xs` — the array to mutate.
- `i` — 1-based index to insert before (three-argument form only). Must
  satisfy `1 <= i <= #xs + 1`.
- `val` — the value to insert.

**Return value:** none; mutates `xs` in place.

**Error conditions:** raises if `i` is out of the valid range in the
three-argument form.

```lua
local xs = {1, 2, 3}
table.insert(xs, 4)        -- xs == {1, 2, 3, 4}
table.insert(xs, 1, 0)     -- xs == {0, 1, 2, 3, 4}
```

## `table.remove`

Removes and returns an element from an array, shifting later elements
down.

**Signature:** `table.remove(xs: table, i?: number) -> any`

**Parameters:**
- `xs` — the array to mutate.
- `i` — 1-based index to remove. Defaults to `#xs` (the last element).

**Return value:** the removed value, or `nil` if `xs` is empty (default
`i`) or `i` is out of range.

**Error conditions:** none — an out-of-range `i` returns `nil` rather
than raising.

```lua
local xs = {1, 2, 3}
table.remove(xs)      -- 3, xs == {1, 2}
table.remove(xs, 1)   -- 1, xs == {2}
```

## `table.find`

Finds the index of the first occurrence of a value in an array.

**Signature:** `table.find(xs: table, val: any) -> number?`

**Parameters:**
- `xs` — the array to search.
- `val` — the value to look for, compared with `==`.

**Return value:** the 1-based index of the first match, or `nil` if not
found.

**Error conditions:** none.

```lua
local xs = {"a", "b", "c"}
table.find(xs, "b")   -- 2
table.find(xs, "z")   -- nil
```

## `table.sort`

Sorts an array's array part in place.

**Signature:** `table.sort(xs: table, cmp?: (a: any, b: any) -> boolean) -> ()`

**Parameters:**
- `xs` — the array to sort, mutated in place.
- `cmp` — optional comparator; `cmp(a, b)` returns `true` if `a` should
  sort before `b`. Defaults to `<` (numeric/lexicographic ascending).

**Return value:** none.

**Error conditions:** raises if elements are not mutually comparable
under the comparator (e.g. mixing numbers and strings with the default
comparator).

```lua
local xs = {3, 1, 2}
table.sort(xs)                              -- xs == {1, 2, 3}
table.sort(xs, function(a, b) return a > b end)  -- xs == {3, 2, 1}
```

## `table.concat`

Joins an array's elements into a single string.

**Signature:** `table.concat(xs: table, sep?: string) -> string`

**Parameters:**
- `xs` — array of strings or numbers (anything directly stringable).
- `sep` — separator inserted between elements. Defaults to `""`.

**Return value:** the joined string. An empty array yields `""`.

**Error conditions:** raises if an element is not a string or number.

```lua
local xs = {"a", "b", "c"}
table.concat(xs, ", ")   -- "a, b, c"
```

## `table.clone`

Shallow-copies a table.

**Signature:** `table.clone(t: table) -> table`

**Parameters:**
- `t` — the table to copy.

**Return value:** a new table with the same keys and values as `t`.
Nested tables are shared (not deep-copied) between the original and the
clone.

**Error conditions:** none.

```lua
local a = {1, 2, {3}}
local b = table.clone(a)
b[1] = 99
-- a[1] is still 1; a[3] and b[3] are the same nested table
```

## Map-style access

Maps use the same `table` type, addressed by non-array keys.

| Idiom | Effect |
| --- | --- |
| `m[key]` | Read (`nil` if absent). |
| `m[key] = value` | Set (or `nil` to remove the key). |
| `m.key` | Dot form of `m["key"]`, for string keys that are valid identifiers. |
| `for k, v in pairs(m) do ... end` | Iterate every key/value pair (order unspecified). |
| `for i, v in ipairs(xs) do ... end` | Iterate the array part in order, stopping at the first `nil`. |

```lua
local m = {a = 1, b = 2}
for k, v in pairs(m) do
    print(k, v)
end
m.a = nil   -- remove key "a"
```

There is no built-in "does this key exist" beyond `m[key] ~= nil`:

```lua
local function keys(m)
    local out = {}
    for k in pairs(m) do
        table.insert(out, k)
    end
    return out
end
```
