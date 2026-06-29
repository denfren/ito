# `yaml` — YAML

| Function / method | Effect |
| --- | --- |
| `yaml.parse(text)` | Parse a single YAML document into native Luau values (lossy). |
| `yaml.to_string(value)` | Serialize a Luau value to canonical block YAML (lossy). |
| `yaml.parse_multi(text)` | Parse a multi-document stream into an array of values. |
| `yaml.to_string_multi(values)` | Serialize an array of values to a multi-document stream. |
| `yaml.edit(text)` | Parse into a format-preserving, editable cursor over the source. |
| `cursor:kind()` | The cursor's node kind, as a string: `"stream"`, `"mapping"`, `"sequence"`, `"scalar"`, or `"null"`. |
| `cursor:is_stream()` / `:is_mapping()` / `:is_sequence()` / `:is_scalar()` / `:is_null()` | Whether the cursor is that kind. |
| `cursor:len()` | Entry/item/document count (mapping, sequence, stream). |
| `cursor:to_string()` | Node's source text (stream renders the whole, possibly-mutated tree). |
| `cursor:keys()` | Mapping: array of decoded key strings. |
| `cursor:values()` | Mapping: array of value cursors, in source order. |
| `cursor:entries()` | Mapping: array of `YamlEntry` handles. |
| `cursor:contains(key)` | Mapping: whether `key` is present. |
| `cursor:get(key_or_index)` | Explicit alternative to `cursor[key_or_index]`. |
| `cursor[key_or_index]` | Bracket indexing (mapping by string key, sequence/stream by 1-based integer). |
| `cursor:mappings()` / `:sequences()` / `:scalars()` | Sequence: elements filtered to that kind. |
| `cursor:doc()` | Stream: root cursor of the first document (errors if empty). |
| `cursor:docs()` | Stream: array of every document's root cursor. |
| `cursor:read()` | Scalar: decode to a native Luau value. |
| `cursor:write(value)` | Scalar: replace the value. |
| `cursor:try_make_unquoted()` / `:make_unquoted()` | Scalar: drop quotes if safe as a plain scalar. |
| `cursor:try_make_single_quoted()` / `:make_single_quoted()` | Scalar: convert to single-quoted. |
| `cursor:comment()` / `:set_comment(text)` / `:clear_comment()` | Leading (`#` block above) comment. |
| `cursor:trailing_comment()` / `:set_trailing_comment(text)` / `:clear_trailing_comment()` | Same-line trailing comment. |
| `cursor:rename_key(old, new)` | Mapping: rename a key in place. |
| `cursor:append(...)` | Mapping `(key, value)` / sequence `(value)`: add after the last item. |
| `cursor:insert_at(index, ...)` | Mapping `(index, key, value)` / sequence `(index, value)`: add before position `index`. |
| `cursor:append_or_update(key, value)` | Mapping: replace in place if present, else append. |
| `cursor:remove(key)` | Mapping: remove by key, returns whether it was found. |
| `cursor:remove_at(index)` | Mapping/sequence: remove by 1-based position. |
| `entry:key()` / `:value()` / `:key_node()` | `YamlEntry`: decoded key string / value cursor / key's own scalar cursor. |

```lua
local cfg = yaml.parse(fs.read("config.yml"))
print(cfg.name)
for _, tag in ipairs(cfg.tags) do print(tag) end
```

`parse` decodes with the YAML 1.2 Core schema: mappings → tables
(keyed), sequences → tables (sequential), and plain scalars are typed as
numbers, booleans, or [`ito.null`](ito.md) (`~`/`null`); quoted and
block scalars stay strings. An empty value (`key:`) and an empty stream
both decode to `ito.null`.

## Not a round-trip

`parse` and `to_string` are lossy: `to_string` writes the library's own
canonical two-space-indented block layout, so comments, flow style, and
the original spacing of a parsed document are not preserved. They exist
for *reading* config to make decisions and *generating* config from
scratch. To edit a file in place while keeping its style — comments,
indentation, flow vs. block — use `yaml.edit` instead, which is backed
by a lossless concrete syntax tree (the `yamlito` crate) rather than the
plain value model `parse`/`to_string` use.

## Format-preserving edits (`yaml.edit`)

`yaml.edit(text)` parses the source into a shared, mutable concrete
syntax tree and returns a single **stream cursor** over its root. Every
kind of node — stream, mapping, sequence, scalar — is represented by the
same Luau userdata type (`YamlCursor`); a method that doesn't apply to
the cursor's current kind (e.g. `:read()` on a mapping) fails with a
runtime error naming what was expected instead of silently doing
nothing. Navigation (`stream[0]`, `m[key]`, `s[i]`) resolves the step
against the live tree and errors immediately if the path is missing —
there is no lazy "maybe missing" chain to check. All cursors reached
from one `yaml.edit` call share the same underlying tree, so a mutation
through any one of them is visible through every other, including the
original stream handle.

```lua
local stream = yaml.edit(fs.read("config.yml"))
local doc = stream:doc()
doc["database"]["port"]:write(5433)
doc["database"]["host"]:set_comment("primary replica")
fs.write("config.yml", stream:to_string())
```

**Always call `:to_string()` on the top-level stream handle** — the
value `yaml.edit` returned — to get the whole document back. That is
the one cursor guaranteed to reflect every edit made anywhere in the
tree. A sub-cursor's own `:to_string()` returns only its own node's text
as of when that cursor was created; fine for reading a value, not the
handle to write back to disk.

There is no `yaml` global surface for reading/writing comments or
structural edits outside `yaml.edit`'s cursor — `comment.rs` (which
implements the comment accessors) and the CST edit primitives in
`edit.rs` are internal machinery behind the cursor methods, not
separately exposed.

### Cursor kinds

Every kind answers `:kind()` and its matching `:is_*()` predicate (see
below); the rest of a kind's methods:

- **Stream** — the whole document stream. `:doc()`, `:len()` (document
  count), `stream[i]` (i-th document's root, 1-based), `:docs()`.
- **Mapping** — `m[key]`, `:len()`, `:keys()`, `:values()`, `:entries()`,
  `:contains(key)`, `:rename_key(old, new)`, and the structural edits
  `:append`/`:append_or_update`/`:insert_at`/`:remove`/`:remove_at`.
- **Sequence** — `s[i]` (1-based), `:len()`, `:mappings()`/`:sequences()`/
  `:scalars()`, and the structural edits `:append`/`:insert_at`/
  `:remove_at`.
- **Scalar** — `:read()`, `:write(value)`, and the quoting helpers.
- **Null** — returned for navigation into an empty document; behaves as
  a null object (collection methods return empty results, `:len()` is
  `0`, comment reads are `""`); mutating methods on it error.

Bracket indexing and `:get(key_or_index)` are equivalent; `:get` exists
for when a variable name would collide with a method name. A mapping
key must be a string; a sequence/stream index must be an integer
(1-based, matching Lua's own indexing convention).

Every cursor, of any kind, answers `:kind()` and the matching `:is_*()`
predicate — useful when a script does not already know what a
navigation step landed on:

```lua
local v = doc["value"]
if v:is_scalar() then
    print(v:read())
elseif v:is_sequence() then
    print(v:len() .. " items")
end
```

`:kind()` and `:is_*()` read the same underlying tag, so they can never
disagree; pick whichever reads better at the call site — a chain of
`elseif`s over `:kind()`, or a single `:is_mapping()` check.

## `yaml.parse(text)`

Parse a single YAML document into native Luau values.

**Signature:** `yaml.parse(text: string) -> value`

**Parameters:**
- `text` — YAML source, one document.

**Returns:** a Luau value — table (mapping/sequence), string, number,
boolean, or `ito.null`.

**Errors:**
- The text is not valid YAML.
- The text contains a multi-document stream (`---` separators) — use
  `yaml.parse_multi`.
- The document contains an anchor alias (`*name`) — not supported. An
  anchor *definition* on a value (`&name`) is accepted; the value is
  read normally, the anchor itself is ignored.

```lua
local cfg = yaml.parse(fs.read("config.yml"))
print(cfg.name)
```

## `yaml.to_string(value)`

Serialize a Luau value to canonical block YAML text.

**Signature:** `yaml.to_string(value: any) -> string`

**Parameters:**
- `value` — any Luau value representable as YAML: `nil`/`ito.null`,
  boolean, number, string, or a table (array-like → sequence,
  map-like → mapping, nested arbitrarily).

**Returns:** a string — two-space-indented block YAML, always ending in
a newline. Scalars are quoted only where a plain scalar would be
misread (looks like a bool/number/null, contains special characters,
etc.).

**Errors:** the value contains something with no YAML representation
(e.g. a function).

```lua
local out = yaml.to_string({name = "web", ports = {80, 443}})
fs.write("config.yml", out)
```

## `yaml.parse_multi(text)`

Parse a multi-document YAML stream (`---`-separated) into an array of
values.

**Signature:** `yaml.parse_multi(text: string) -> {value}`

**Parameters:**
- `text` — YAML source, any number of documents.

**Returns:** an array with one decoded value per document — a
single-document input yields a one-element array, an empty stream
yields an empty array.

**Errors:** the text is not valid YAML.

```lua
local docs = yaml.parse_multi(fs.read("k8s.yml"))
for _, doc in ipairs(docs) do print(doc.kind) end
```

## `yaml.to_string_multi(values)`

Serialize an array of Luau values to a multi-document YAML stream.

**Signature:** `yaml.to_string_multi(values: {any}) -> string`

**Parameters:**
- `values` — array of values, each independently serializable as by
  `yaml.to_string`.

**Returns:** a string with each element rendered as a document,
separated by `---`.

**Errors:** an element contains something with no YAML representation.

```lua
local manifest = yaml.to_string_multi({
  {kind = "Service", metadata = {name = "web"}},
  {kind = "Deployment", metadata = {name = "web"}},
})
fs.write("k8s.yml", manifest)
```

## `yaml.edit(text)`

Parse YAML source into a format-preserving, editable cursor.

**Signature:** `yaml.edit(text: string) -> cursor` (a Stream cursor)

**Parameters:**
- `text` — YAML source (single- or multi-document stream).

**Returns:** a Stream cursor over the parsed tree's root.

**Errors:** the text is not valid YAML.

```lua
local stream = yaml.edit(fs.read("config.yml"))
```

## `cursor:kind()`

The cursor's node kind, as a string.

**Signature:** `cursor:kind() -> string`

**Returns:** one of `"stream"`, `"mapping"`, `"sequence"`, `"scalar"`,
`"null"`.

**Errors:** never — every cursor kind, including Null, answers this.

```lua
print(doc["value"]:kind())  -- e.g. "scalar"
```

## `cursor:is_stream()` / `:is_mapping()` / `:is_sequence()` / `:is_scalar()` / `:is_null()`

Whether the cursor is that kind.

**Signature:** `cursor:is_stream() -> boolean` (and so on, one per kind)

**Returns:** `true` if `cursor:kind()` equals the corresponding string,
else `false`. Reads the same tag `:kind()` does, so the two can never
disagree.

**Errors:** never.

```lua
local v = doc["value"]
if v:is_scalar() then
    print(v:read())
end
```

## `cursor:len()`

Element count of a collection cursor.

**Signature:** `cursor:len() -> number`

**Returns:** entry count (mapping), item count (sequence), document
count (stream). `0` for a Null cursor.

**Errors:** called on a Scalar cursor.

```lua
local n = stream:doc()["ports"]:len()
```

## `cursor:to_string()`

Render the cursor's own node as source text.

**Signature:** `cursor:to_string() -> string`

**Returns:** the raw source text of the node (mapping/sequence/scalar),
the whole rendered (possibly mutated) tree for a Stream cursor, or `""`
for Null. See [Format-preserving edits](#format-preserving-edits-yamledit)
for why the top-level stream handle is the one to call this on before
writing back to disk.

```lua
fs.write("config.yml", stream:to_string())
```

## `cursor:keys()`

List a mapping's keys.

**Signature:** `cursor:keys() -> {string}`

**Returns:** array of decoded key strings, in source order. `{}` for
Null.

**Errors:** called on a non-mapping cursor (other than Null).

```lua
for _, k in ipairs(doc:keys()) do print(k) end
```

## `cursor:values()`

List a mapping's values.

**Signature:** `cursor:values() -> {cursor}`

**Returns:** array of value cursors, in source order. `{}` for Null.

**Errors:** called on a non-mapping cursor (other than Null).

```lua
for _, v in ipairs(doc:values()) do print(v:to_string()) end
```

## `cursor:entries()`

Iterate a mapping as key/value pairs.

**Signature:** `cursor:entries() -> {YamlEntry}`

**Returns:** array of `YamlEntry` handles, in source order. `{}` for
Null. Each entry has `:key()` (decoded key string), `:value()` (value
cursor), `:key_node()` (a Scalar cursor at the key itself, useful for
reading/setting a comment on the key).

**Errors:** called on a non-mapping cursor (other than Null).

```lua
for _, entry in ipairs(doc:entries()) do
  print(`{entry:key()} = {entry:value():to_string()}`)
end
```

## `cursor:contains(key)`

Test whether a mapping has a key.

**Signature:** `cursor:contains(key: string) -> boolean`

**Parameters:**
- `key` — the key to look up.

**Returns:** `true`/`false`. `false` for Null.

**Errors:** called on a non-mapping cursor (other than Null).

```lua
if doc["metadata"]:contains("labels") then ... end
```

## `cursor:get(key_or_index)`

Explicit alternative to bracket indexing.

**Signature:** `cursor:get(key_or_index: string | number) -> cursor`

**Parameters:**
- `key_or_index` — a string key (mapping) or 1-based integer index
  (sequence/stream).

**Returns:** the cursor at that step; a Null cursor if indexing through
an already-Null cursor.

**Errors:** wrong key type for the cursor kind (e.g. an integer index
into a mapping); key not found (mapping); index out of range
(sequence/stream); called on a Scalar cursor (not indexable).

```lua
local port = doc:get("service"):get("port")
```

## `cursor[key_or_index]`

Bracket indexing — the same operation as `:get`.

**Signature:** `cursor[key: string]` (mapping), `cursor[i: number]`
(sequence, stream)

**Returns/Errors:** identical to `:get`.

```lua
local port = doc["service"]["port"]
local first_doc = stream[1]
```

## `cursor:mappings()` / `cursor:sequences()` / `cursor:scalars()`

Sequence element selectors, filtered by kind.

**Signature:** `cursor:mappings() -> {cursor}`, likewise for
`:sequences()`/`:scalars()`

**Returns:** array of element cursors of just that kind, in source
order. `{}` for Null. `:scalars()` includes alias nodes.

**Errors:** called on a non-sequence cursor (other than Null).

```lua
for _, m in ipairs(items:mappings()) do
  print(m["name"]:read())
end
```

## `cursor:doc()`

Step from a Stream cursor to its first document's root.

**Signature:** `cursor:doc() -> cursor`

**Returns:** the root cursor (Mapping/Sequence/Scalar/Null) of the first
document.

**Errors:** called on a non-Stream cursor; the stream has no documents.

```lua
local doc = yaml.edit(fs.read("config.yml")):doc()
```

## `cursor:docs()`

List every document's root cursor.

**Signature:** `cursor:docs() -> {cursor}`

**Returns:** array of root cursors, one per document. `{}` for Null or
an empty stream.

**Errors:** called on a non-Stream cursor (other than Null).

```lua
for _, doc in ipairs(stream:docs()) do print(doc:to_string()) end
```

## `cursor:read()`

Decode a scalar cursor to a native Luau value.

**Signature:** `cursor:read() -> value`

**Returns:** the scalar decoded with the same YAML 1.2 Core schema
`yaml.parse` uses (number, boolean, `ito.null`, or string).

**Errors:** called on a non-Scalar cursor; the node cannot be decoded as
a scalar.

```lua
local replicas = doc["spec"]["replicas"]:read()   -- 3
```

## `cursor:write(value)`

Replace a scalar's value.

**Signature:** `cursor:write(value: nil | boolean | number | string)`

**Parameters:**
- `value` — the new scalar value. Chooses plain or single-quoted output
  by the same safety rules `yaml.to_string` uses — a string that looks
  like a bool/number/null is quoted automatically.

**Returns:** nothing.

**Errors:** called on a non-Scalar cursor; `value` is a table or other
non-scalar (cannot be written as a YAML scalar).

```lua
doc["spec"]["replicas"]:write(4)
doc["spec"]["image"]:write("nginx:1.27")
```

## `cursor:try_make_unquoted()` / `cursor:make_unquoted()`

Drop a scalar's quotes if it is safe to write as a plain scalar.

**Signature:** `cursor:try_make_unquoted() -> boolean`,
`cursor:make_unquoted()`

**Returns:** `try_make_unquoted` returns `true` if converted, `false` if
not possible (quotes stay). `make_unquoted` returns nothing.

**Errors:** `make_unquoted` errors if the conversion isn't possible (a
single-quoted value containing `''`, a double-quoted value containing a
backslash escape, or content that isn't valid as a plain scalar).
`try_make_unquoted` never errors for these cases — it just returns
`false`. Both error if called on a non-Scalar cursor.

```lua
if not doc["name"]:try_make_unquoted() then
  print("value needs quoting")
end
```

## `cursor:try_make_single_quoted()` / `cursor:make_single_quoted()`

Convert a scalar to single-quoted style.

**Signature:** `cursor:try_make_single_quoted() -> boolean`,
`cursor:make_single_quoted()`

**Returns:** `try_make_single_quoted` returns `true` if converted,
`false` if not possible (already single-quoted, or a double-quoted
value containing a backslash escape). `make_single_quoted` returns
nothing.

**Errors:** `make_single_quoted` errors when the `try_` form would
return `false`. Both error if called on a non-Scalar cursor.

```lua
doc["password"]:make_single_quoted()
```

## `cursor:comment()`

Read a node's leading comment.

**Signature:** `cursor:comment() -> string`

**Returns:** the `#` block attached directly above the enclosing
mapping/sequence entry, or (for a Stream cursor) the block before the
first `---`. `""` if there is none, or for Null.

```lua
print(doc["port"]:comment())
```

## `cursor:set_comment(text)`

Set a node's leading comment.

**Signature:** `cursor:set_comment(text: string)`

**Parameters:**
- `text` — the comment body. Each `\n`-separated line becomes one `#`
  line.

**Returns:** nothing.

**Errors:** the cursor sits inside a flow-style mapping/sequence
(`{…}`/`[…]`) — there is nowhere to hang a line comment without
reflowing to block style first; called on a Null cursor. For a Stream
cursor, requires the source to have an explicit `---` marker to attach
before.

```lua
doc["database"]["host"]:set_comment("primary replica")
```

## `cursor:clear_comment()`

Remove a node's leading comment.

**Signature:** `cursor:clear_comment()`

**Returns:** nothing. A no-op if there was no comment, if the cursor is
inside a flow construct, or if the cursor is Null.

```lua
doc["port"]:clear_comment()
```

## `cursor:trailing_comment()`

Read a node's same-line trailing comment.

**Signature:** `cursor:trailing_comment() -> string`

**Returns:** the same-line `#` comment after the enclosing entry. `""`
if none, for Null, or (errors instead, see below) for a Stream cursor.

**Errors:** called on a Stream cursor (documents have no same-line
trailing-comment concept).

```lua
print(doc["port"]:trailing_comment())
```

## `cursor:set_trailing_comment(text)`

Set a node's same-line trailing comment.

**Signature:** `cursor:set_trailing_comment(text: string)`

**Parameters:**
- `text` — the comment body. Must not contain `\n` (it is same-line).

**Returns:** nothing.

**Errors:** `text` contains `\n`; the cursor sits inside a flow-style
construct; called on a Stream cursor.

```lua
doc["port"]:set_trailing_comment("default HTTPS")
```

## `cursor:clear_trailing_comment()`

Remove a node's trailing comment.

**Signature:** `cursor:clear_trailing_comment()`

**Returns:** nothing.

**Errors:** called on a Stream cursor (or Scalar/Mapping/Sequence cursor
inside a flow construct — see note); a no-op otherwise if there was no
comment.

```lua
doc["port"]:clear_trailing_comment()
```

## `cursor:rename_key(old_key, new_key)`

Rename a mapping key in place, keeping its position and value.

**Signature:** `cursor:rename_key(old_key: string, new_key: string) -> boolean`

**Parameters:**
- `old_key` — the key to rename.
- `new_key` — the replacement key text (quoted automatically if needed).

**Returns:** `true` if `old_key` was found and renamed, `false` if
absent.

**Errors:** called on a non-mapping cursor.

```lua
doc["metadata"]:rename_key("name", "app_name")
```

## `cursor:append(...)`

Add a new entry/item after the last one.

**Signature:** mapping — `cursor:append(key: string, value: any)`;
sequence — `cursor:append(value: any)`

**Parameters:**
- `key` (mapping only) — the new entry's key.
- `value` — a scalar or a table (array/map, nested arbitrarily); tables
  are serialized and spliced so the result always reparses.

**Returns:** nothing. The cursor is refreshed in place to the mapping's
or sequence's new node, so the same handle stays valid for further
edits.

**Errors:** (mapping) `key` already exists — a YAML mapping cannot
repeat a key, use `append_or_update` instead; called on a Scalar/Stream
cursor.

```lua
meta:append("namespace", "prod")     -- mapping
ports:append(8443)                    -- sequence
```

## `cursor:insert_at(index, ...)`

Insert a new entry/item before a given position.

**Signature:** mapping — `cursor:insert_at(index: number, key: string,
value: any)`; sequence — `cursor:insert_at(index: number, value: any)`

**Parameters:**
- `index` — 1-based position to insert before; an index past the end
  clamps to append.
- `key` (mapping only) — the new entry's key.
- `value` — a scalar or table, as for `:append`.

**Returns:** nothing. Cursor refreshed in place.

**Errors:** `index < 1`; (mapping) `key` already exists; called on a
Scalar/Stream cursor.

```lua
ports:insert_at(1, 80)
meta:insert_at(1, "namespace", "prod")
```

## `cursor:append_or_update(key, value)`

Set a mapping entry, replacing it in place if present, appending
otherwise.

**Signature:** `cursor:append_or_update(key: string, value: any)`

**Parameters:**
- `key` — the entry's key.
- `value` — a scalar or table, as for `:append`.

**Returns:** nothing. Cursor refreshed in place. If `key` existed, its
position is kept; never produces a duplicate key.

**Errors:** called on a non-mapping cursor.

```lua
meta:append_or_update("name", "frontend")   -- set, whether or not present
```

## `cursor:remove(key)`

Remove a mapping entry by key.

**Signature:** `cursor:remove(key: string) -> boolean`

**Parameters:**
- `key` — the entry's key.

**Returns:** `true` if found and removed, `false` if absent. Cursor
refreshed in place on removal.

**Errors:** called on a non-mapping cursor.

```lua
meta:remove("annotations")
```

## `cursor:remove_at(index)`

Remove a mapping entry or sequence item by position.

**Signature:** `cursor:remove_at(index: number)`

**Parameters:**
- `index` — 1-based position. Out-of-range is a no-op.

**Returns:** nothing. Cursor refreshed in place.

**Errors:** `index < 1`; called on a Scalar/Stream cursor.

```lua
ports:remove_at(3)
```

## Notes on structural edits

Adding/removing preserves the existing layout: block sequences/mappings
stay block (dashes/`key:` lines), flow ones stay flow (`[...]`/`{...}`),
including a multiline flow list's per-item indentation and trailing-comma
style. Removing the last remaining item of a *block* sequence or entry
of a *block* mapping collapses it to flow `[]`/`{}` (a block collection
cannot be empty); the cursor then points at that flow node, so a
following `:append` grows it again.

```lua
local stream = yaml.edit(fs.read("config.yml"))
local ports = stream:doc()["service"]["ports"]
ports:append(8443)
ports:insert_at(1, 80)
ports:remove_at(3)
fs.write("config.yml", stream:to_string())
```

## `entry:key()`

Read a `YamlEntry`'s decoded key.

**Signature:** `entry:key() -> string`

**Returns:** the key as a decoded string.

```lua
for _, entry in ipairs(doc:entries()) do print(entry:key()) end
```

## `entry:value()`

Get a `YamlEntry`'s value cursor.

**Signature:** `entry:value() -> cursor`

**Returns:** the cursor at the entry's value.

```lua
entry:value():write("new value")
```

## `entry:key_node()`

Get a `YamlEntry`'s key as its own scalar cursor.

**Signature:** `entry:key_node() -> cursor` (Scalar)

**Returns:** a Scalar cursor at the key token itself — useful for
attaching a comment to the key or checking its quote style.

```lua
entry:key_node():set_comment("renamed from `hostname`")
```
