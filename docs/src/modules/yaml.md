# `yaml` — YAML

| Function | Effect |
| --- | --- |
| `yaml.parse(text, [name])` | Parse a single YAML document into native Luau values (lossy). |
| `yaml.to_string(value)` | Serialize a Luau value to canonical block YAML (lossy). |
| `yaml.parse_multi(text, [name])` | Parse a multi-document stream into an array of values. |
| `yaml.to_string_multi(values)` | Serialize an array of values to a multi-document stream. |
| `yaml.edit(text, [name])` | Parse into a format-preserving, editable cursor over the source. |
| `cursor:kind()` | The resolved node's kind, as a string: `"stream"`, `"mapping"`, `"sequence"`, `"scalar"`, or `"alias"`. |
| `cursor:is_stream()` / `:is_mapping()` / `:is_sequence()` / `:is_scalar()` / `:is_alias()` | Whether the cursor is that kind. |
| `cursor:exists()` | Whether the cursor's path resolves. |
| `cursor:len()` | Entry/item/document count (mapping, sequence, stream). |
| `cursor:to_string()` | Node's source text (stream renders the whole, possibly-mutated tree). |
| `cursor:raw()` | Node's exact source text, as an opaque handle. |
| `cursor:keys()` | Mapping: array of decoded key strings. |
| `cursor:values()` | Mapping or sequence: array of value cursors, in source order. |
| `cursor:entries()` | Mapping: array of `YamlEntry` handles. |
| `cursor:contains(key)` | Mapping: whether `key` is present. |
| `cursor:get(key_or_index)` | Explicit alternative to `cursor[key_or_index]`. |
| `cursor[key_or_index]` | Bracket indexing (mapping by string key, sequence/stream by 1-based integer). |
| `cursor:mappings()` / `:sequences()` / `:scalars()` | Sequence: elements filtered to that kind. |
| `cursor:doc()` | Stream: root cursor of the first document (errors if empty). |
| `cursor:docs()` | Stream: array of every document's root cursor. |
| `cursor:read()` | Scalar: decode to a native Luau value. |
| `cursor:write(value)` | Replace the node's value (scalar or collection); a key node renames the key. |
| `cursor:quote_style()` | Scalar: its current style, as a string. |
| `cursor:unquote()` / `:single_quote()` / `:double_quote()` / `:auto_quote()` | Scalar: convert to that style; returns whether it happened. |
| `cursor:comment()` / `:set_comment(text)` / `:clear_comment()` | Leading (`#` block above) comment. |
| `cursor:trailing_comment()` / `:set_trailing_comment(text)` / `:clear_trailing_comment()` | Same-line trailing comment. |
| `cursor:rename_key(old, new)` | Mapping: rename a key in place. |
| `cursor:add(...)` | Mapping `(key, value)` / sequence `(value)`: add after the last item; stream `(value)`: append a document. |
| `cursor:add_at(index, ...)` | Mapping `(index, key, value)` / sequence `(index, value)`: add before position `index`. |
| `cursor:update(...)` | Self `(update_fn)` / mapping `(key, update_fn)` / by-position `(index, update_fn)`: read-modify-write a scalar. |
| `cursor:update_at(index, update_fn)` | Sequence element or mapping entry by position: read-modify-write. |
| `cursor:update_all(update_fn)` | Every mapping value or sequence element: read-modify-write in place. |
| `cursor:upsert(key, upsert_fn)` | Mapping: update in place if present, else add the function's result. |
| `cursor:upsert_at(index, key, upsert_fn)` | Mapping: as `upsert`, but a new entry lands at `index`. |
| `cursor:remove()` | Removes the element this cursor itself addresses. |
| `cursor:remove(key)` | Mapping: remove by key, returns whether it was found. |
| `cursor:remove_at(index)` | Mapping/sequence: remove by 1-based position. |
| `entry:key()` / `:value()` / `:key_node()` | `YamlEntry`: decoded key string / value cursor / key's own scalar cursor. |
| `cursor:try_*(...)` | Non-raising sibling of `kind`, `len`, `to_string`, `read`, `write`, `raw` and `remove`. |

## Example

```lua
local cfg = yaml.parse(fs.read("config.yml"))
print(cfg.name)
for _, tag in ipairs(cfg.tags) do print(tag) end
```

`parse` decodes with the YAML 1.2 Core schema: mappings → tables
(keyed), sequences → tables (sequential), and plain scalars are typed as
numbers, booleans, or the global [`null`](table.md#the-global-null) (`~`/`null`); quoted and
block scalars stay strings. An empty value (`key:`) and an empty stream
both decode to `null`. Sequences carry the array marker (`table.array`,
see [`json`](json.md)), so an empty sequence round-trips; an empty Luau
table writes as `{}`. Mapping keys are written sorted. Writing raises for
mixed/sparse tables, invalid UTF-8, functions and cycles, naming the path.
`parse`, `parse_multi` and `edit` take an optional second argument `name`,
added to parse errors (`yaml parse error in <name>: ...`).

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
the node the cursor resolves to (e.g. `:read()` on a mapping) fails with
a runtime error naming what was expected instead of silently doing
nothing. All cursors reached from one `yaml.edit` call share the same
underlying tree, so a mutation through any one of them is visible
through every other, including the original stream handle.

```lua
local stream = yaml.edit(fs.read("config.yml"))
local doc = stream:doc()
doc["database"]["port"]:write(5433)
doc["database"]["host"]:set_comment("primary replica")
fs.write("config.yml", stream:to_string())
```

**Call `:to_string()` on the top-level stream handle** — the value
`yaml.edit` returned — to get the whole document back. A sub-cursor's
`:to_string()` renders only *its own node*: current as of the call, but
a fragment, so it is what you want for inspecting a value and not the
handle to write back to disk.

### A cursor is a path, not a node

A cursor holds the shared tree plus the **path** to a node within it.
Traversal (`stream[i]`, `m[key]`, `s[i]`, `:doc()`) only extends that
path: it never touches the tree, so **it never fails**. Every read and
every write resolves the path against the tree as it is *at that
moment*.

This is the same model [`hcl.edit`](hcl.md) uses, and it is what makes a
cursor survive edits made through another one:

```lua
local doc = yaml.edit(fs.read("config.yml")):doc()
local port = doc["database"]["port"]
doc:remove("legacy")     -- an unrelated entry goes away
port:write(5433)         -- `port` still addresses the right node
```

Two consequences worth knowing:

- **A path is a coordinate, not an identity.** `s[3]` means "whatever is
  at index 3 *now*", so after `s:remove_at(1)` it addresses what used to
  be the fourth element. Key-addressed cursors (`m["port"]`) are stable
  under sibling insertion and removal; index-addressed ones are not.
  This is why `:values()` and `:entries()` hand back key-addressed
  cursors.
- **A missing key surfaces at the operation, not at the navigation.**
  `doc["typo"]` is fine on its own; `doc["typo"]:read()` is what raises,
  naming the path it got through (`key not found: typo (at database)`).
  Use `:exists()` to ask directly, or a `try_*` method to swallow it:

```lua
if doc["database"]:exists() then ... end
local port = doc["database"]["port"]:try_read()   -- nil when absent
```

Comments and structural edits are only available through the cursor's
methods; there is no separate `yaml` function for them.

An **integer never addresses a mapping entry**, even one whose key looks
numeric (`1: x`): it is a position, valid on sequences and streams. Use a
string (`m["1"]`) for such a key; indexing a mapping by integer raises an
error saying so.

### Cursor kinds

Every kind answers `:kind()` and its matching `:is_*()` predicate (see
below); the rest of a kind's methods:

- **Stream** — the whole document stream. `:doc()`, `:len()` (document
  count), `stream[i]` (i-th document's root, 1-based), `:docs()`.
- **Mapping** — `m[key]`, `:len()`, `:keys()`, `:values()`, `:entries()`,
  `:contains(key)`, `:rename_key(old, new)`, and the structural edits
  `:add`/`:add_at`/`:update`/`:update_at`/`:update_all`/`:upsert`/
  `:upsert_at`/`:remove`/`:remove_at`.
- **Sequence** — `s[i]` (1-based), `:len()`, `:mappings()`/`:sequences()`/
  `:scalars()`, and the structural edits `:add`/`:add_at`/`:update`/
  `:update_at`/`:update_all`/`:remove`/`:remove_at`.
- **Scalar** — `:read()`, `:write(value)`, and the quoting helpers. An
  implicit null (`key:` with no value) is a scalar; it reads as the global
  [`null`](table.md#the-global-null), and `:write(...)` works on it like on any scalar.
- **Alias** — an `*anchor` reference. It carries no scalar token of its
  own, so it answers `:kind()`, `:exists()` and the comment accessors,
  but not `:read()` or the quoting helpers.

Every cursor also answers `:exists()`, which resolves the path and
reports whether it is there. There is no null-object cursor: a path that
does not resolve raises on use rather than quietly answering `0`/`{}`.

Bracket indexing and `:get(key_or_index)` are equivalent; `:get` exists
for when a variable name would collide with a method name. A mapping
key must be a string; a sequence/stream index must be an integer
(1-based, matching Luau's own indexing convention). Which container an
integer indexes — a sequence's elements or a stream's documents — is
settled when the path resolves, from the kind of the node it lands on.

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

## `yaml.parse`

Parse a single YAML document into native Luau values.

**Signature:** `yaml.parse(text: string, name: string?) -> value`

**Parameters:**
- `text` — YAML source, one document.
- `name` — optional, added to parse errors.

**Returns:** a Luau value — table (mapping/sequence), string, number,
boolean, or `null`.

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

## `yaml.to_string`

Serialize a Luau value to canonical block YAML text.

**Signature:** `yaml.to_string(value: any) -> string`

**Parameters:**
- `value` — any Luau value representable as YAML: `null`,
  boolean, number, string, or a table (array-like → sequence,
  map-like → mapping, nested arbitrarily). An empty table is a mapping;
  use `table.array()` for an empty sequence.

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

## `yaml.parse_multi`

Parse a multi-document YAML stream (`---`-separated) into an array of
values.

**Signature:** `yaml.parse_multi(text: string, name: string?) -> {value}`

**Parameters:**
- `text` — YAML source, any number of documents.
- `name` — optional, added to parse errors.

**Returns:** an array with one decoded value per document — a
single-document input yields a one-element array, an empty stream
yields an empty array.

**Errors:** the text is not valid YAML, or a document contains an anchor
alias (`*name`) — aliases are rejected here as in `yaml.parse`.

```lua
local docs = yaml.parse_multi(fs.read("k8s.yml"))
for _, doc in ipairs(docs) do print(doc.kind) end
```

## `yaml.to_string_multi`

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

## `yaml.edit`

Parse YAML source into a format-preserving, editable cursor.

**Signature:** `yaml.edit(text: string, name: string?) -> cursor` (a Stream cursor)

**Parameters:**
- `text` — YAML source (single- or multi-document stream). Empty text is
  valid: a stream with no document (`:len()` is `0`); `stream:add(value)`
  creates the first one.
- `name` — optional, added to parse errors.

**Returns:** a Stream cursor over the parsed tree's root.

**Errors:** the text is not valid YAML.

```lua
local stream = yaml.edit(fs.read("config.yml"))
```

## `cursor:kind` / `:try_kind`

The resolved node's kind, as a string.

**Signature:** `cursor:kind() -> string`, `cursor:try_kind() -> string?`

**Returns:** one of `"stream"`, `"mapping"`, `"sequence"`, `"scalar"`,
`"alias"`. An implicit null (`key:` with no value) is a `"scalar"`.

**Errors:** the cursor's path does not resolve. Kind is a property of
the node, so there has to *be* one — `:try_kind()` returns `nil`
instead, and `:exists()` asks the question directly.

```lua
print(doc["value"]:kind())  -- e.g. "scalar"
```

## `cursor:is_stream` / `:is_mapping` / `:is_sequence` / `:is_scalar` / `:is_alias`

Whether the cursor is that kind.

**Signature:** `cursor:is_stream() -> boolean` (and so on, one per kind)

**Returns:** `true` if `cursor:kind()` equals the corresponding string,
else `false`. Reads the same value `:kind()` does, so the two can never
disagree.

**Errors:** the cursor's path does not resolve — as for `:kind()`, and
for the same reason.

```lua
local v = doc["value"]
if v:is_scalar() then
    print(v:read())
end
```

## `cursor:exists`

Whether the cursor's path resolves against the document as it is now.

**Signature:** `cursor:exists() -> boolean`

**Returns:** `true` if every step of the path is present, else `false`.

**Errors:** never — this is the method that answers the question the
others raise about.

```lua
if doc["database"]["replica"]:exists() then
    print("replica configured")
end
```

## `cursor:len`

Element count of a collection cursor.

**Signature:** `cursor:len() -> number`

**Returns:** entry count (mapping), item count (sequence), document
count (stream). Only documents that have a root node count — the same
ones `:docs()`, `:doc()` and `stream[i]` see — so a comments-only stream
has length `0`.

**Errors:** called on a Scalar or Alias cursor; the cursor's path does
not resolve. `:try_len()` returns `nil` in either case.

```lua
local n = stream:doc()["ports"]:len()
```

## `cursor:to_string`

Render the cursor's own node as source text.

**Signature:** `cursor:to_string() -> string`

**Returns:** the raw source text of the node (mapping/sequence/scalar),
or the whole rendered (possibly mutated) tree for a Stream cursor.
Resolved fresh, so it reflects every edit made so far. See
[Format-preserving edits](#format-preserving-edits-yamledit) for why the
top-level stream handle is the one to call this on before writing back
to disk.

**Errors:** the cursor's path does not resolve. `:try_to_string()`
returns `nil` instead.

```lua
fs.write("config.yml", stream:to_string())
```

## `cursor:raw` / `:try_raw`

The node's exact source text, as an opaque handle rather than a plain
string.

**Signature:** `cursor:raw() -> RawNode`, `cursor:try_raw() -> RawNode?`

**Returns:** a `RawNode` — `tostring(raw)` (or `raw:to_string()`) gives
back the same text `:to_string()` would. The two exist for different
reasons: `:to_string()` is for reading/displaying; `:raw()` reserves a
distinct type for a verbatim value, the way `hcl.edit`'s
[`raw()`](hcl.md) does — an opaque handle a future verbatim-write escape
hatch can recognise unambiguously against an ordinary string. No method
accepts a `RawNode` back (there is no `:write(raw)` splice); `:raw()` is a
read-only capture of a node's exact text.

**Errors:** the cursor's path does not resolve. `:try_raw()` returns
`nil` instead.

```lua
local text = tostring(doc["password"]:raw())
```

## `cursor:keys`

List a mapping's keys.

**Signature:** `cursor:keys() -> {string}`

**Returns:** array of decoded key strings, in source order.

**Errors:** called on a non-mapping cursor; the path does not resolve.

```lua
for _, k in ipairs(doc:keys()) do print(k) end
```

## `cursor:values`

List a mapping's values or a sequence's elements.

**Signature:** `cursor:values() -> {cursor}`

**Returns:** array of value cursors, in source order. For a mapping each
is addressed by **key**, so it stays valid when a sibling entry is added
or removed; for a sequence each is addressed by **position** and shifts
with its earlier siblings.

**Errors:** called on a cursor that is neither mapping nor sequence; the
path does not resolve.

```lua
for _, v in ipairs(doc:values()) do print(v:to_string()) end
```

## `cursor:entries`

Iterate a mapping as key/value pairs.

**Signature:** `cursor:entries() -> {YamlEntry}`

**Returns:** array of `YamlEntry` handles, in source order. Each entry
has `:key()` (decoded key string), `:value()` (value cursor),
`:key_node()` (a Scalar cursor at the key itself, useful for
reading/setting a comment on the key). Both cursors are addressed by
**key**, so they stay valid when a sibling entry is added or removed;
if *this* entry is removed, `:value():exists()` reports `false`.

**Errors:** called on a non-mapping cursor; the path does not resolve.

```lua
for _, entry in ipairs(doc:entries()) do
  print(`{entry:key()} = {entry:value():to_string()}`)
end
```

## `cursor:contains`

Test whether a mapping has a key.

**Signature:** `cursor:contains(key: string) -> boolean`

**Parameters:**
- `key` — the key to look up.

**Returns:** `true`/`false`.

**Errors:** called on a non-mapping cursor; the path does not resolve.
(`contains` asks about a key *within* this mapping; to ask whether the
mapping itself is there, use `:exists()`.)

```lua
if doc["metadata"]:contains("labels") then ... end
```

## `cursor:get`

Explicit alternative to bracket indexing.

**Signature:** `cursor:get(key_or_index: string | number) -> cursor`

**Parameters:**
- `key_or_index` — a string key (mapping) or 1-based integer index
  (sequence/stream).

**Returns:** a new cursor whose path is this one plus that step. The
document is not consulted, so nothing is checked yet.

**Errors:** `key_or_index` is neither a string nor an integer, or the
integer is `< 1`. Notably **not** an error here: a key that is absent, an
index past the end, or a step out of a scalar — traversal does not
resolve, so those surface at the next read or write, naming the path
they got through. `:exists()` checks up front.

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

## `cursor:mappings` / `cursor:sequences` / `cursor:scalars`

Sequence element selectors, filtered by kind.

**Signature:** `cursor:mappings() -> {cursor}`, likewise for
`:sequences()`/`:scalars()`

**Returns:** array of element cursors of just that kind, in source
order; `:scalars()` includes alias nodes. Each is addressed by
**position**, so — unlike the mapping accessors — it shifts if an
earlier element is removed.

**Errors:** called on a non-sequence cursor; the path does not resolve.

```lua
for _, m in ipairs(items:mappings()) do
  print(m["name"]:read())
end
```

## `cursor:doc`

Step from a Stream cursor to its first document's root.

**Signature:** `cursor:doc() -> cursor`

**Returns:** the root cursor of the first document — the same cursor as
`stream[1]`.

**Errors:** called on a non-Stream cursor; the stream has no documents.
This is the one traversal that checks: `:doc()` names no index of its
own, so "the stream has no document" is a better answer than a later
`index out of range: 1`.

```lua
local doc = yaml.edit(fs.read("config.yml")):doc()
```

## `cursor:docs`

List every document's root cursor.

**Signature:** `cursor:docs() -> {cursor}`

**Returns:** array of root cursors, one per document (numbered like
`stream[i]`); `{}` for an empty stream. A document with no root node is
skipped.

**Errors:** called on a non-Stream cursor; the path does not resolve.

```lua
for _, doc in ipairs(stream:docs()) do print(doc:to_string()) end
```

## `cursor:read` / `:try_read`

Decode a scalar cursor to a native Luau value.

**Signature:** `cursor:read() -> value`, `cursor:try_read() -> value?`

**Returns:** the scalar decoded with the same YAML 1.2 Core schema
`yaml.parse` uses (number, boolean, `null`, or string). An implicit
null (`key:` with no value) reads as the global `null` sentinel.

**Errors:** called on a non-Scalar cursor (an alias is not a scalar);
the path does not resolve; the node cannot be decoded as a scalar.
`:try_read()` returns `nil` in every one of those cases — which is
different from a successfully read `null` (which is the sentinel, not
`nil`); use `:exists()` when a failure must be told from an absent path.

```lua
local replicas = doc["spec"]["replicas"]:read()   -- 3
```

## `cursor:write` / `:try_write`

Replace the node the cursor addresses.

**Signature:** `cursor:write(value: any)`,
`cursor:try_write(value) -> boolean`

**Parameters:**
- `value` — the new value: `null`/`nil` (writes `null`), a boolean, number,
  string, or a table (written as a mapping or sequence, in the layout
  `yaml.to_string` uses). A scalar over a scalar keeps the node's trivia,
  anchor/tag and **quote style** (a string written over `'x'` stays
  single-quoted when it can be); otherwise the style follows the safety
  rules `yaml.to_string` uses (plain where unambiguous, else
  single-quoted, else double-quoted). Floats are formatted as by
  `yaml.to_string`. Use the [quoting methods](#scalar-quoting) to force a
  style.
- On a **key node** (`entry:key_node()`), `value` must be a string: the key
  is renamed, exactly like `:rename_key` (duplicates raise).

A collection, an alias or an implicit null (`key:`) is replaced through its
parent (same position, its trailing comment is dropped). The document root
itself cannot be replaced.

**Returns:** nothing. `:try_write` returns whether the write happened.

**Errors:** called on a stream cursor or the document root; the path does
not resolve; `value` has no YAML form (function, mixed table, ...).
`:try_write` returns `false` instead of raising.

```lua
doc["spec"]["replicas"]:write(4)
doc["spec"]["image"]:write("nginx:1.27")
```

## Scalar quoting

There is one method per quoting style, plus `auto_quote()` for
"whatever is narrowest and still round-trips":

| Method | Result |
| --- | --- |
| `unquote()` | `value` |
| `single_quote()` | `'value'` |
| `double_quote()` | `"value"` |
| `auto_quote()` | the narrowest of the three that survives a re-read |

Each one **decodes the scalar and re-encodes it**, so any pair of styles
is reachable — `'it''s'` becomes `"it's"`, and `"a\tb"` becomes a
double-quoted scalar rather than being refused. The value is preserved
exactly; only its spelling changes.

Each of the four returns a **boolean** — whether the conversion
happened — rather than raising, since there is no other natural return
value for a restyle:

- **`unquote`** — most strings aren't safe as plain scalars. A value
  that would re-read as a bool, number or null (`true`, `42`, `~`), one
  containing `: `, ` #`, a flow terminator (`,` `[` `]` `{` `}`) or a
  control character, one starting with an indicator (`-` `*` `&` `!` `|`
  `>` `%` `#` …), one with leading or trailing whitespace, and one a YAML
  1.1 reader would misread (`yes`, `010`, `1:30`) all return `false`.
- **`single_quote`** — single-quoted scalars have no escape for control
  characters, and are still line-folded on re-read, so a value containing
  `\n`/`\t` or having leading/trailing whitespace returns `false`.
- **`double_quote`** and **`auto_quote`** — always return `true` for a
  representable value: double-quoted style has an escape for every
  character.

All four **raise** on a non-scalar receiver (that is a usage error, not
a "can't represent" case), and all four decline a **block scalar**
(`|`/`>`) by returning `false`: its value is multi-line text whose
indentation is part of the document's layout, so re-emitting it as a
single quoted token would reflow the file rather than restyle a value.
Asking for the style a scalar already has is a no-op that returns
`true`, not a failure.

There are no `try_` twins for these four: the boolean return already
*is* the non-raising form.

### `cursor:quote_style`

A scalar's current style.

**Signature:** `cursor:quote_style() -> string`

**Returns:** one of `"plain"`, `"single"`, `"double"`, `"literal"`
(`|`), `"folded"` (`>`), or `"none"` for a scalar with no value token of
its own: an implicit null (`key:`).

**Errors:** called on a non-Scalar cursor.

```lua
if doc["password"]:quote_style() == "plain" then
  doc["password"]:single_quote()
end
```

### `cursor:unquote` / `cursor:single_quote` / `cursor:double_quote` / `cursor:auto_quote`

Convert a scalar to the named style.

**Signature:** `cursor:unquote() -> boolean`,
`cursor:single_quote() -> boolean`, `cursor:double_quote() -> boolean`,
`cursor:auto_quote() -> boolean`

**Returns:** `true` if the scalar is now in that style, `false` if the
style cannot represent the value (or the scalar is a block scalar). On
`false` the document is left byte-for-byte unchanged.

**Errors:** called on a non-Scalar cursor. The representability
failures return `false` instead of raising.

```lua
doc["password"]:single_quote()   -- password: 'hunter2'
doc["motd"]:double_quote()       -- motd: "line\nbreak"
doc["name"]:auto_quote()         -- name: 'quoted' -> name: quoted

if not doc["name"]:unquote() then
  print("value needs quoting")
end
```

`auto_quote` is the one to reach for when normalizing a file: it strips
quotes that aren't doing anything, while keeping the ones that are.

```lua
-- 'plain' -> plain, but 'true' and '42' keep their quotes (they would
-- otherwise re-read as a boolean and a number).
for _, v in ipairs(doc["env"]:values()) do
  v:auto_quote()
end
```

## `cursor:comment`

Read a node's leading comment.

**Signature:** `cursor:comment() -> string`

**Returns:** the `#` block attached directly above the enclosing
mapping/sequence entry, or (for a Stream cursor) the block before the
first `---`. `""` if there is none.

**Errors:** the cursor's path does not resolve.

```lua
print(doc["port"]:comment())
```

## `cursor:set_comment`

Set a node's leading comment.

**Signature:** `cursor:set_comment(text: string)`

**Parameters:**
- `text` — the comment body. Each `\n`-separated line becomes one `#`
  line.

**Returns:** nothing.

**Errors:** the cursor sits inside a flow-style mapping/sequence
(`{…}`/`[…]`) — there is nowhere to hang a line comment without
reflowing to block style first; the path does not resolve. For a Stream
cursor, requires the source to have an explicit `---` marker to attach
before.

```lua
doc["database"]["host"]:set_comment("primary replica")
```

## `cursor:clear_comment`

Remove a node's leading comment.

**Signature:** `cursor:clear_comment()`

**Returns:** nothing. A no-op if there was no comment or if the cursor
is inside a flow construct.

**Errors:** the cursor's path does not resolve.

```lua
doc["port"]:clear_comment()
```

## `cursor:trailing_comment`

Read a node's same-line trailing comment.

**Signature:** `cursor:trailing_comment() -> string`

**Returns:** the same-line `#` comment after the enclosing entry. `""`
if none.

**Errors:** called on a Stream cursor (documents have no same-line
trailing-comment concept); the path does not resolve.

```lua
print(doc["port"]:trailing_comment())
```

## `cursor:set_trailing_comment`

Set a node's same-line trailing comment.

**Signature:** `cursor:set_trailing_comment(text: string)`

**Parameters:**
- `text` — the comment body. Must not contain `\n` (it is same-line).

**Returns:** nothing.

**Errors:** `text` contains `\n`; the cursor sits inside a flow-style
construct; called on a Stream cursor; the path does not resolve.

```lua
doc["port"]:set_trailing_comment("default HTTPS")
```

## `cursor:clear_trailing_comment`

Remove a node's trailing comment.

**Signature:** `cursor:clear_trailing_comment()`

**Returns:** nothing.

**Errors:** called on a Stream cursor; the path does not resolve. A
no-op otherwise if there was no comment, or if the cursor sits inside a
flow construct.

```lua
doc["port"]:clear_trailing_comment()
```

## `cursor:rename_key`

Rename a mapping key in place, keeping its position and value.

**Signature:** `cursor:rename_key(old_key: string, new_key: string) -> boolean`

**Parameters:**
- `old_key` — the key to rename.
- `new_key` — the replacement key text (quoted automatically if needed).

**Returns:** `true` if `old_key` was found and renamed, `false` if
absent.

**Errors:** called on a non-mapping cursor; the path does not resolve;
`new_key` already exists in the mapping (`duplicate key`).
`entry:key_node():write(new_key)` is the same operation.

Note that a cursor already held on the *value* of `old_key` is
key-addressed, so it stops resolving after the rename — re-navigate
through `new_key`.

```lua
doc["metadata"]:rename_key("name", "app_name")
```

## `cursor:add`

Add a new entry/item after the last one.

**Signature:** mapping — `cursor:add(key: string, value: any)`;
sequence — `cursor:add(value: any)`; stream — `cursor:add(value: any)`

**Parameters:**
- `key` (mapping only) — the new entry's key.
- `value` — a scalar or a table (array/map, nested arbitrarily); tables
  are serialized and spliced so the result always reparses. `null` (or an
  explicit `nil`) is a value. The argument count is checked: too few or
  too many raise.
- On a **stream**, `value` becomes a new document appended after the last
  one (the first one, for an empty stream such as `yaml.edit("")`). The
  tree is re-parsed; existing comments survive.

**Returns:** nothing. The cursor's path still names this collection, so
the same handle keeps working — the next call resolves it to the
rebuilt node.

**Errors:** (mapping) `key` already exists — a YAML mapping cannot
repeat a key, use `upsert` instead; wrong argument count; called on a
scalar cursor; the path does not resolve.

```lua
meta:add("namespace", "prod")     -- mapping
ports:add(8443)                    -- sequence
```

## `cursor:add_at`

Insert a new entry/item before a given position.

**Signature:** mapping — `cursor:add_at(index: number, key: string,
value: any)`; sequence — `cursor:add_at(index: number, value: any)`

**Parameters:**
- `index` — 1-based position to insert before; an index past the end
  clamps to append.
- `key` (mapping only) — the new entry's key.
- `value` — a scalar or table, as for `:add`.

**Returns:** nothing. The cursor keeps working, as for `:add`.

**Errors:** `index < 1`; (mapping) `key` already exists; called on a
Scalar/Stream cursor; the path does not resolve.

Inserting shifts the position of every later element, so any
**index-addressed** cursor into this collection (from `s[i]`, or from
`:mappings()`/`:sequences()`/`:scalars()`) now names its neighbour.

```lua
ports:add_at(1, 80)
meta:add_at(1, "namespace", "prod")
```

## `cursor:update`

Read-modify-write a scalar in place.

**Signature:** self — `cursor:update(update_fn: (value) -> value?)`;
mapping — `cursor:update(key: string, update_fn)`; by position —
`cursor:update(index: number, update_fn)`

**Parameters:**
- `key` (mapping) — the entry to update.
- `index` — 1-based position; works for a sequence element *or* a
  mapping entry, the container settled the same way bracket indexing
  settles it.
- `update_fn` — called with the entry's current decoded value (as
  `:read()` would return it); its return value is written back, or the
  entry is **removed** if it returns `nil`.

**Returns:** nothing.

**Errors:** the target (self, or the named/positioned entry) does not
resolve to a **scalar** — a mapping/sequence/stream target raises,
`update` only operates on scalars; called with no arguments; the first
argument is neither an index nor a key.

```lua
doc["spec"]["replicas"]:update(function(v) return v + 1 end)
doc:update("replicas", function(v) return v + 1 end)
doc:update(2, function(v) return v + 1 end)
```

## `cursor:update_at`

Read-modify-write a sequence element or mapping entry by position.

**Signature:** `cursor:update_at(index: number, update_fn: (value) -> value?)`

**Parameters:**
- `index` — 1-based position.
- `update_fn` — as for `:update`.

**Returns:** nothing.

**Errors:** as for `:update`'s by-position form; `index` out of range.

```lua
ports:update_at(1, function(v) return v + 1 end)
```

## `cursor:update_all`

Read-modify-write every value of a mapping, or every element of a
sequence, in place.

**Signature:** `cursor:update_all(update_fn: (value) -> value?)`

**Parameters:**
- `update_fn` — called once per entry/element, in document order, with
  its current decoded value; a `nil` return removes that entry/element,
  any other return replaces it.

**Returns:** nothing.

**Errors:** called on a Scalar/Stream cursor; the path does not
resolve; an entry/element is not itself a scalar (`update_all` shares
`update`'s scalar-only restriction).

```lua
doc["env"]:update_all(function(v)
  if v == "unset" then return nil end
  return v
end)
```

## `cursor:upsert`

Update a mapping entry in place if present, or add the function's
result if not.

**Signature:** `cursor:upsert(key: string, upsert_fn: (value?) -> value?)`

**Parameters:**
- `key` — the entry's key.
- `upsert_fn` — called with the entry's current decoded value if `key`
  is present, or `nil` if it is absent. Its return value is written
  back (if present) or **added** (if absent), unless it returns `nil`
  while `key` was absent, in which case nothing is added.

**Returns:** nothing. If `key` existed, its position is kept; never
produces a duplicate key. Returning `nil` **deletes** an existing entry
(as for `:update`); for an absent key it adds nothing.

**Errors:** called on a non-mapping cursor; the path does not resolve;
the existing entry (when present) is not a scalar.

```lua
meta:upsert("name", function(v)
  if v == nil then return "frontend" end   -- was absent: add it
  return v                                   -- was present: keep it
end)
```

## `cursor:upsert_at`

As `:upsert`, but a newly-added entry lands at a given position instead
of the end.

**Signature:** `cursor:upsert_at(index: number, key: string, upsert_fn: (value?) -> value?)`

**Parameters:**
- `index` — 1-based position to insert at, if `key` is absent (ignored
  if `key` is already present — its existing position is kept).
- `key`, `upsert_fn` — as for `:upsert`.

**Returns:** nothing.

**Errors:** as for `:upsert`; `index < 1`.

```lua
meta:upsert_at(1, "namespace", function(v)
  if v == nil then return "prod" end
  return v
end)
```

## `cursor:remove`

Remove the element this cursor addresses, or a mapping entry by key.

**Signature:** `cursor:remove() -> boolean`, `cursor:remove(key: string) -> boolean`

**Parameters:**
- *(none)* — removes the element this cursor itself addresses, resolved
  against its own parent (works whether the cursor was reached by key or
  by index).
- `key` — removes that entry from the mapping this cursor points at.

**Returns:** `true` if found and removed, `false` if absent (including a
position past the end of the collection). The cursor keeps working either
way.

**Errors:** called on a non-mapping cursor (the `key` form); the path
does not resolve; the no-arg form on a cursor with no parent (the
document root). `:try_remove(...)` returns `false` instead of raising.

```lua
doc["metadata"]["annotations"]:remove()   -- self-removal
meta:remove("annotations")                 -- by key
```

## `cursor:remove_at`

Remove a mapping entry or sequence item by position.

**Signature:** `cursor:remove_at(index: number)`

**Parameters:**
- `index` — 1-based position.

**Returns:** nothing. The cursor keeps working.

**Errors:** `index < 1`; `index` past the end (`index out of range`);
called on a Scalar/Stream cursor; the path does not resolve.

As with `:add_at`, removing shifts every later position, so an
**index-addressed** cursor into this collection now names its
neighbour — and one that named the last element stops resolving.

```lua
ports:remove_at(3)
```

## Notes on structural edits

Adding/removing preserves the existing layout: block sequences/mappings
stay block (dashes/`key:` lines), flow ones stay flow (`[...]`/`{...}`),
including a multiline flow list's per-item indentation and trailing-comma
style. Removing the last remaining item of a *block* sequence or entry
of a *block* mapping collapses it to flow `[]`/`{}` (a block collection
cannot be empty); the collection's path is unchanged, so the cursor
resolves to that flow node and a following `:add` grows it again.

Because a cursor resolves its path on every call rather than holding a
node, edits made through one cursor are immediately visible through
every other — and a cursor is never left dangling by an edit made
elsewhere. What *can* change under it is which node its path names:
positions shift (see `:add_at`/`:remove_at`), while keys do not.

```lua
local stream = yaml.edit(fs.read("config.yml"))
local ports = stream:doc()["service"]["ports"]
ports:add(8443)
ports:add_at(1, 80)
ports:remove_at(3)
fs.write("config.yml", stream:to_string())
```

## `cursor:try_kind` / `:try_len` / `:try_to_string` / `:try_raw` / `:try_read` / `:try_write` / `:try_remove`

Non-raising siblings of the calls that resolve a path.

**Signature:** as the method they mirror, but returning `nil` (for the
readers) or `false` (for `:try_write`/`:try_remove`) where the original
would raise.

**Returns:** the original's result on success; otherwise `nil`/`false`
for *any* failure — a path that does not resolve, a wrong-kind receiver,
or an undecodable value. They do not distinguish between those; use
`:exists()` and `:kind()` when the difference matters.

**Errors:** never.

Only these seven have `try_` forms. The rest either cannot fail once the
path resolves, or are mutations whose failure a script should not
swallow silently — this now includes `:add`, `:add_at`, `:update`,
`:update_at`, `:update_all`, `:upsert`, and `:upsert_at`, none of which
have a `try_` twin. (The scalar-quoting `unquote`/`single_quote`/
`double_quote`/`auto_quote` are a different case: their boolean return
*is* their non-raising form, so they need no separate `try_` twin at
all — see [Scalar quoting](#scalar-quoting).)

```lua
local port = doc["service"]["port"]:try_read() or 8080
```

## `entry:key`

Read a `YamlEntry`'s decoded key.

**Signature:** `entry:key() -> string`

**Returns:** the key as a decoded string. This is the entry's *address*
rather than a snapshot, so it still answers after the entry is removed
(at which point `entry:value():exists()` is `false`).

```lua
for _, entry in ipairs(doc:entries()) do print(entry:key()) end
```

## `entry:value`

Get a `YamlEntry`'s value cursor.

**Signature:** `entry:value() -> cursor`

**Returns:** the cursor at the entry's value, addressed by **key** — so
it stays valid when a sibling entry is added or removed.

```lua
entry:value():write("new value")
```

## `entry:key_node`

Get a `YamlEntry`'s key as its own scalar cursor.

**Signature:** `entry:key_node() -> cursor` (Scalar)

**Returns:** a Scalar cursor at the key token itself — useful for
attaching a comment to the key or checking its quote style. Addressed
by key, like `:value()`.

```lua
entry:key_node():set_comment("renamed from `hostname`")
```
