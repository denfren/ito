# `hcl` — HCL

| Function | Effect |
| --- | --- |
| `hcl.parse(text)` | Parse HCL into a native Luau value (lossy). |
| `hcl.to_string(value)` | Serialize a Luau value to HCL text (lossy). |
| `hcl.builder()` | Start building an HCL document (a body). |
| `hcl.block(ident)` | Start building an HCL block. |
| `hcl.expr(text)` | Parse a raw HCL expression, emitted verbatim/unquoted. |
| `hcl.ident(name)` | A bare identifier/variable-reference expression. |
| `hcl.edit(text)` | Parse HCL into an editable, format-preserving handle. |

| Builder method | Effect |
| --- | --- |
| `body:with_attribute(key, value)` | Append an attribute; returns `body`. |
| `body:with_block(block)` | Append a nested block; returns `body`. |
| `body:with_comment(text)` | Attach a leading comment; returns `body`. |
| `body:to_string()` | Render the body to HCL text. |
| `block:with_label(text)` | Append a quoted label; returns `block`. |
| `block:with_attribute(key, value)` | Append an attribute; returns `block`. |
| `block:with_block(child)` | Append a nested block; returns `block`. |
| `block:with_comment(text)` | Attach a leading comment; returns `block`. |

| `HclDoc` (from `hcl.edit`) | Effect |
| --- | --- |
| `h[key]` / `h[index]` | Navigate to an attribute/label-less block, or an array element. |
| `h:attr(key)` / `h:attribute(key)` | Navigate to an attribute, disambiguating against a block. |
| `h:block(ident[, label...][, nth])` | Navigate to a block by identifier/labels/occurrence. |
| `h:exists()` | Whether the cursor's path resolves. |
| `h:read()` | Read the value at the path as a native Luau value (literals only). |
| `h:read_raw()` | Read the expression at the path verbatim (any expression). |
| `h:write(value)` | Replace the value (or block) at the path. |
| `h:add(...)` | Add an attribute/block/array element. |
| `h:remove(...)` | Remove the element at the path, or a named child. |
| `h:len()` | Item count of a body/block/object/array. |
| `h:attributes()` / `h:attrs()` | Handles to each attribute of a body/block. |
| `h:attribute_keys()` / `h:attr_keys()` | The attribute names of a body/block. |
| `h:blocks([ident[, label...]])` | Handles to matching nested blocks. |
| `h:block_types()` | The distinct block identifiers in a body/block. |
| `h:block_labels(ident)` | The distinct label tuples for blocks named `ident`. |
| `h:block_count(ident[, label...])` | Count of blocks matching a label pattern. |
| `h:is_array()` / `h:is_object()` / `h:is_expr()` / `h:is_block()` / `h:is_body()` | Type checks on the cursor's target. |
| `h:to_string()` | Render the (sub)document back to HCL text. |
| `h:try_*(...)` | Non-raising sibling of every call above. |

## Motivating example

```lua
-- Bump an AMI id in place, keeping every comment and blank line intact.
local h = hcl.edit(fs.read("main.tf"))
h:block("resource", "aws_instance", "web"):attr("ami"):write("ami-0123456789")
fs.write("main.tf", h:to_string())
```

## General rules

- **Two independent surfaces.** `hcl.parse`/`hcl.to_string` are a lossy
  value round-trip, like `json`/`toml`: comments and formatting are not
  preserved. `hcl.edit` is a format-preserving CST cursor: untouched
  text (comments, blank-line spacing, indentation) survives verbatim,
  and only the parts you `:write`/`:add`/`:remove` change. Use `parse`
  to read a config file as data; use `edit` to patch one in place.
- **The builder (`hcl.builder`/`hcl.block`) generates new HCL text**
  from scratch, fluently. Its blocks can also be spliced into an
  `hcl.edit` document via `:write`/`:add`, which is the only way to
  create a *new* block inside an editable handle (the cursor itself has
  no "create block" primitive of its own).
- **Builders are handles, not values.** `hcl.builder()`/`hcl.block(...)`
  return `Rc<RefCell<..>>`-backed objects; every `with_*` method mutates
  the receiver in place and returns it, so `local x = a:with_block(b)`
  aliases `a` rather than copying it.
- **Attribute values.** Any Luau value (scalars, tables/arrays) passed
  to `:with_attribute`, `:write`, or `:add` is converted through JSON:
  a plain string is emitted **quoted**. To emit a bare keyword,
  traversal, function call, or other computed expression, wrap it with
  `hcl.expr` or `hcl.ident` first — those pass through **verbatim**.
- **The `hcl.edit` cursor is a path, not a live reference.** Every
  traversal call (`h[key]`, `:attr()`, `:block()`) returns a *new*
  handle carrying an extended path over the *same* shared document; the
  original handle is unchanged. `:read`/`:write`/`:add`/`:remove`
  resolve that path against the document at call time.
- **A cursor lands on one of five kinds of target**: the document root
  or a nested block's contents (**body**), a **block** header+body, an
  **object** or **array** expression, or a scalar **expr**. `:is_body()`
  / `:is_block()` / `:is_object()` / `:is_array()` / `:is_expr()` report
  which; `:is_expr()` is also true for object/array targets.
- **Only literal expressions read back as Luau values.** `:read()`
  converts strings, numbers, bools, `null`, arrays, and objects — never
  `var.x`, function calls, conditionals, or templates (those raise, or
  yield `nil` from `:try_read()`). Such expressions are left untouched
  by everything except an explicit `:write`, and survive `:to_string()`
  unchanged. `:read_raw()` reads *any* expression (literal or computed)
  as an opaque value that round-trips into `:write`/`:add`.
- **Every fallible `HclDoc` method has a `try_` sibling.** Read-like
  `try_*` return `nil` on failure; write-like `try_*` return `false`.
  Use them when a path may legitimately be absent.
- **Label patterns.** Wherever `:block()`/`:blocks()`/`:block_count()`
  take labels, they are a *pattern*, matched position-by-position: a
  literal string matches that exact label, `"*"` matches exactly one
  label (any value), `"**"` matches zero or more labels (any values).
  Omitting the labels argument entirely matches **any** labels
  (equivalent to `{"**"}`); pass an explicit empty table `{}` to match
  only a block with *zero* labels. The pattern is anchored at both
  ends — without a trailing `"**"` its length must equal the block's
  label count.
- **1-based indices** everywhere a script supplies one (array element,
  `nth` block occurrence), matching Luau's own convention.
- **Comments are leading-only.** `:with_comment(text)` on the builder
  requires every line of `text` to already start with `#` or `//` (the
  marker is never added for you); anything else is a runtime error.
  There is no comment support on the `hcl.edit` cursor — comments there
  are preserved automatically as part of the surrounding text, not
  something a script attaches after the fact.

## `hcl.parse(text)`

Parse HCL text into a native Luau value (lossy — like `json.parse`).

**Signature:** `hcl.parse(text: string) -> value`

**Parameters:**
- `text` — HCL source text.

**Returns:** a Luau value: tables for objects/blocks and arrays,
strings, numbers, booleans, `nil`/`ito.null` per the usual JSON-ish
mapping. Blocks with labels nest under their label path.

**Errors:** raises `"hcl parse error: ..."` on malformed HCL.

```lua
local doc = hcl.parse(fs.read("main.tf"))
print(doc.resource.aws_instance.web.ami)
```

## `hcl.to_string(value)`

Serialize a Luau value to HCL text (lossy — like `toml.to_string`; does
not necessarily round-trip with `hcl.parse`).

**Signature:** `hcl.to_string(value: any) -> string`

**Parameters:**
- `value` — any Luau value convertible through the JSON bridge (tables,
  strings, numbers, bools, nil).

**Returns:** the HCL text.

**Errors:** raises `"hcl serialize error: ..."` if the value can't be
represented (e.g. unsupported nesting).

```lua
local text = hcl.to_string({ region = "us-east-1" })
```

## `hcl.builder()`

Start building an HCL document (the top-level body).

**Signature:** `hcl.builder() -> body`

**Returns:** a fresh `body` handle with no attributes or blocks.

```lua
local doc = hcl.builder():with_attribute("region", "us-east-1")
fs.write("main.tf", doc:to_string())
```

## `hcl.block(ident)`

Start building an HCL block.

**Signature:** `hcl.block(ident: string) -> block`

**Parameters:**
- `ident` — the block type identifier (e.g. `"resource"`, `"variable"`).
  Validated as an HCL identifier, but only at render time (`:to_string()`
  / splicing into `hcl.edit`) — `hcl.block("")` itself does not raise.

**Returns:** a fresh `block` handle with no labels, attributes, or
blocks.

**Errors:** rendering (`:to_string()`, or splicing via `:write`/`:add`
into an `hcl.edit` handle) raises `"hcl identifier error: ..."` if
`ident` is not a valid HCL identifier.

```lua
local b = hcl.block("resource"):with_label("aws_instance"):with_label("web")
```

## `body:with_attribute(key, value)` / `block:with_attribute(key, value)`

Append an attribute (`key = value`) to a body or block.

**Signature:** `body:with_attribute(key: string, value: any) -> body`
(same shape on `block`, returning `block`)

**Parameters:**
- `key` — the attribute name; validated as an HCL identifier at render
  time.
- `value` — a plain Luau value (converted through JSON, so a string is
  emitted quoted) or an `HclExpr` from `hcl.expr`/`hcl.ident` (emitted
  verbatim).

**Returns:** the receiver (chainable).

**Errors:** at render time, raises `"hcl identifier error: ..."` for an
invalid `key`, or `"hcl value error: ..."` if `value` can't convert.

```lua
local v = hcl.block("variable"):with_label("infra_env")
  :with_attribute("type", hcl.expr("string"))   -- type = string
  :with_attribute("default", "dev")              -- default = "dev"
```

## `body:with_block(block)` / `block:with_block(child)`

Append a nested block.

**Signature:** `body:with_block(block: block) -> body` (same shape on
`block:with_block(child)`, returning `block`)

**Parameters:**
- `block`/`child` — a handle from `hcl.block(...)`.

**Returns:** the receiver (chainable).

```lua
local inner = hcl.block("tags"):with_attribute("Env", "prod")
local res = hcl.block("resource"):with_label("aws_instance"):with_label("web")
  :with_attribute("ami", "ami-123")
  :with_block(inner)
```

## `body:with_comment(text)` / `block:with_comment(text)`

Attach a leading comment.

**Signature:** `body:with_comment(text: string) -> body` (same shape on
`block`, returning `block`)

**Parameters:**
- `text` — one or more lines; every line (after optional leading
  whitespace) must already start with `#` or `//`.

**Returns:** the receiver (chainable).

**Errors:** raises `"hcl comment error: empty comment"` for `""`, or
`"hcl comment error: every line must start with `#` or `//`: ..."` for
any line missing a marker.

Placement depends on the receiver's current state:
- body with **no items yet** → a standalone header comment at the top;
- block with **no inner items yet** → comments the block's own header
  line;
- otherwise → comments the most-recently-added item (attribute or
  block).

```lua
local v = hcl.block("variable"):with_label("aws_account_id")
  :with_attribute("type", hcl.expr("string"))
local doc = hcl.builder()
  :with_block(v)
  :with_comment("# tflint-ignore: terraform_unused_declarations")
fs.write("variables.tf", doc:to_string())
```

```hcl
# tflint-ignore: terraform_unused_declarations
variable "aws_account_id" {
  type = string
}
```

## `block:with_label(text)`

Append a label to a block header.

**Signature:** `block:with_label(text: string) -> block`

**Parameters:**
- `text` — the label value; rendered quoted (`Debug`-formatted), so it
  needs no pre-quoting.

**Returns:** `block` (chainable).

```lua
local b = hcl.block("resource"):with_label("aws_instance"):with_label("web")
-- resource "aws_instance" "web" { ... }
```

## `body:to_string()`

Render a built body to HCL text.

**Signature:** `body:to_string() -> string`

**Returns:** the rendered document (attributes and blocks in insertion
order; a blank line is inserted around every block).

**Errors:** raises on an invalid identifier or unconvertible value
introduced by an earlier `with_*` call (validation is deferred to
render time).

```lua
fs.write("main.tf", hcl.builder():with_attribute("region", "us-east-1"):to_string())
```

Note: `block` has no `:to_string()` of its own — render it by attaching
it to a body (`hcl.builder():with_block(b):to_string()`), or splice it
directly into an `hcl.edit` handle via `:write`/`:add`.

## `hcl.expr(text)`

Parse a raw HCL expression (bare keyword, traversal, function call,
array/object literal, ...), to be emitted **verbatim, unquoted** when
used as an attribute value.

**Signature:** `hcl.expr(text: string) -> HclExpr`

**Parameters:**
- `text` — HCL expression source (e.g. `"string"`, `"var.region"`,
  `"max(1, 2)"`, `'["a", "b"]'`).

**Returns:** an opaque `HclExpr` value, usable only as an attribute
value / array element / `hcl.edit` `:write`/`:add` argument — not a
general-purpose serializer.

**Errors:** raises `"hcl expression error: ..."` on malformed input.

```lua
local v = hcl.block("variable"):with_label("x")
  :with_attribute("type", hcl.expr("list(string)"))
-- type = list(string)
```

## `hcl.ident(name)`

A bare identifier / variable reference expression — the single-token
shorthand for `hcl.expr`.

**Signature:** `hcl.ident(name: string) -> HclExpr`

**Parameters:**
- `name` — an identifier, validated (e.g. `"string"`, `"each"`).

**Returns:** an opaque `HclExpr` value, same use sites as `hcl.expr`.

**Errors:** raises `"hcl identifier error: ..."` if `name` is not a
valid identifier.

```lua
hcl.block("variable"):with_label("kind"):with_attribute("type", hcl.ident("string"))
```

## `hcl.edit(text)`

Parse HCL into an editable, format-preserving handle.

**Signature:** `hcl.edit(text: string) -> HclDoc`

**Parameters:**
- `text` — HCL source text.

**Returns:** an `HclDoc` cursor rooted at the document body (an empty
navigation path — `:is_body()` is `true`).

**Errors:** raises `"hcl parse error: ..."` on malformed HCL.

```lua
local h = hcl.edit(fs.read("main.tf"))
```

## Traversal: `h[key]` / `h[index]`

Bracket-syntax navigation: a string key reaches an attribute or a
**label-less** block named `key`; an integer index reaches an array
element.

**Signature:** `h[key: string] -> HclDoc`, `h[index: integer] -> HclDoc`

**Parameters:**
- `key` — attribute name, or block identifier (only matches a block
  with **zero** labels — a labelled block needs `:block(...)`).
- `index` — 1-based array element position.

**Returns:** a new `HclDoc` with the path extended by one segment; does
not resolve immediately (resolution happens on `:read`/`:write`/etc.).

**Errors:** resolving a path where the key matches **both** an
attribute and a label-less block raises `"Ambiguous key '...'..."` —
use `:attr(key)` to disambiguate. A non-string/non-integer key raises
immediately.

```lua
local ami = h["resource"]["aws_instance"]  -- not valid: labelled blocks need :block()
local ami = h:block("resource", "aws_instance", "web")["ami"]
```

## `h:attr(key)` / `h:attribute(key)`

Navigate to an attribute named `key`, unambiguously (skips the
block-name check `h[key]` does).

**Signature:** `h:attr(key: string) -> HclDoc`

**Parameters:**
- `key` — attribute name.

**Returns:** a new `HclDoc` with an `Attr` path segment appended.
Aliases: `attribute` is identical.

```lua
h:block("resource", "aws_instance", "web"):attr("ami"):write("ami-456")
```

## `h:block(ident[, label...][, nth])`

Navigate to a nested block by identifier and label pattern.

**Signature:** `h:block(ident: string, ...) -> HclDoc`

**Parameters:**
- `ident` — the block type identifier (required, must be a string).
- variadic tail, several accepted shapes:
  - *(nothing)* — match any labels (`{"**"}`), no `nth`.
  - one to three string labels — exact match, no `nth`.
  - a single table of string labels — same, as an array.
  - any of the above followed by a trailing integer — that integer is
    the 1-based `nth` occurrence among matches.
  - a lone integer — any labels, at `nth`.

**Returns:** a new `HclDoc` with a `Block` path segment (`ident`,
resolved labels, optional `nth`) appended.

**Errors:** resolving raises `"Ambiguous block: found N blocks matching
'...'..."` if more than one block matches and no `nth` was given; raises
`NotFound` if resolution finds none. A non-string `ident`, or a
non-string/non-table label, raises immediately.

```lua
h:block("resource", "aws_instance", "web")       -- 2 labels
h:block("resource", {"aws_instance", "web"})     -- same, table form
h:block("data")                                  -- any labels
h:block("data", 2)                               -- any labels, 2nd occurrence (1-based)
h:block("locals", {})                            -- exactly zero labels
```

## `h:exists()`

Whether the cursor's path resolves against the current document.

**Signature:** `h:exists() -> boolean`

**Returns:** `true`/`false`; never raises (a root cursor with an empty
path is always `true`).

```lua
if h:block("provider", "aws"):exists() then ... end
```

## `h:read()`

Read the value at the path as a native Luau value.

**Signature:** `h:read() -> value`

**Returns:** the same value shapes `hcl.parse` produces (strings,
numbers, bools, `ito.null`, tables) for a **literal** attribute value,
array, or object at the cursor.

**Errors:** raises `InvalidType` if the cursor targets a block or the
body itself (only `Expr`/`Object`/`Array` targets are readable), or if
the expression is computed (`var.x`, function call, conditional,
template, ...) rather than literal — those are `UnsupportedHclType`.
Raises `NotFound`/`AmbiguousBlock`/etc. if the path itself doesn't
resolve (see `:block`/`h[key]` above).

```lua
local ami = h:block("resource", "aws_instance", "web"):attr("ami"):read()
```

## `h:read_raw()`

Read the expression at the path **verbatim**, as an opaque value — the
same kind `hcl.expr`/`hcl.ident` produce. Unlike `:read()`, this works
for computed expressions (`var.x`, function calls, `list(string)`,
conditionals) and round-trips straight back into `:write`/`:add`.

**Signature:** `h:read_raw() -> HclExpr`

**Returns:** an opaque `HclExpr`. The cursor must point at an
expression (attribute value / array element / object value), not a
body or block.

**Errors:** raises `InvalidType` if the cursor targets a body or block.

```lua
local t = h:block("variable", "src"):attr("type"):read_raw()
h:block("variable", "dst"):add("type", t)   -- copy the type verbatim
```

## `h:write(value)`

Replace the value at the path.

**Signature:** `h:write(value: any) -> ()`

**Parameters:**
- `value` — a plain Luau value (converted through JSON; strings are
  quoted), an `HclExpr` (emitted verbatim), or — only when the cursor
  points at a **block** — a `block` builder from `hcl.block(...)`, which
  replaces the whole block in place, keeping its position and
  indentation.

**Returns:** nothing.

**Errors:** raises `NotFound` if the target key/index doesn't exist in
the parent; `InvalidType` if the parent isn't a body/block/object/array,
or (for the block-replace form) if the cursor's last path segment isn't
a `Block` segment; `IndexOutOfBounds` for an out-of-range array index.

```lua
h:block("resource", "aws_instance", "web"):attr("ami"):write("ami-0123456789")

-- replace a whole block, keeping its position
h:block("resource", "aws_instance", "web")
  :write(hcl.block("resource"):with_label("aws_instance"):with_label("web")
           :with_attribute("ami", "ami-456"))
```

## `h:add(...)`

Add an attribute, a nested block, or an array element, depending on
argument shape.

**Signature:** `h:add(value)`, `h:add(key, value)`, `h:add(index, value)`

**Parameters** (dispatch by arity/type):
- `h:add(value)` — the cursor must target a body/block (appends a
  nested `block` builder) or an array (appends `value`, converted like
  `:write`).
- `h:add(key: string, value)` — the cursor must target a body/block/
  object; adds an attribute/member named `key`.
- `h:add(index: integer, value)` — the cursor must target an array;
  inserts `value` before the 1-based `index` (out-of-range, including
  `<= 0`, clamps to the nearest end rather than erroring).

**Returns:** nothing.

**Errors:** raises `InvalidType` if the cursor's target doesn't match
the call shape; raises for a wrong argument count (`"add: expected 1 or
2 arguments, got N"`) or a first argument that's neither an integer nor
a string in the 2-arg form.

Added attributes/blocks are indented to match an existing sibling, or
two spaces per nesting level if the body is empty.

```lua
-- append an attribute
h:block("resource", "aws_instance", "web"):add("tags", { Env = "prod" })

-- append a new block to the document
h:add(hcl.block("output"):with_label("id")
        :with_attribute("value", hcl.expr("aws_instance.web.id")))

-- array insert/append
h:attr("ports"):add(8080)        -- append
h:attr("ports"):add(1, 22)       -- insert at position 1
```

## `h:remove(...)`

Remove the element at the cursor's own path, or a named/indexed child.

**Signature:** `h:remove()`, `h:remove(key)`, `h:remove(index)`

**Parameters:**
- *(none)* — removes the element the cursor itself points at (resolved
  against its **parent**); works for an attribute, a block (matched by
  the cursor's own ident/labels/`nth`), an object member, or an array
  element.
- `key: string` — removes a child attribute or block(s) named `key`
  from the body/block/object the cursor points at.
- `index: integer` — removes the 1-based element from the array the
  cursor points at.

**Returns:** nothing.

**Errors:** raises `NotFound` if there's nothing to remove; `InvalidType`
if the cursor/parent shape doesn't match; `IndexOutOfBounds` for a bad
array index; a bad argument type/count raises immediately.

```lua
h:block("resource", "aws_instance", "old"):remove()   -- remove the block itself
h:block("resource", "aws_instance", "web"):remove("ami")  -- remove one attribute
h:attr("ports"):remove(1)                              -- remove array element 1
```

## `h:len()`

Item count of the cursor's target.

**Signature:** `h:len() -> integer`

**Returns:** attribute+block count for a body/block, member count for
an object, element count for an array.

**Errors:** raises `InvalidType` for an `Expr` (scalar) target.

```lua
print(h:block("locals"):len())
```

## `h:attributes()` / `h:attrs()`

Handles to every attribute in a body or block.

**Signature:** `h:attributes() -> {HclDoc}`

**Returns:** an array of `HclDoc` cursors, one per attribute, in
document order. Alias: `attrs`.

**Errors:** raises `InvalidType` if the cursor isn't a body/block.

```lua
for _, a in h:attributes() do print(a:read()) end
```

## `h:attribute_keys()` / `h:attr_keys()`

The attribute names of a body or block.

**Signature:** `h:attribute_keys() -> {string}`

**Returns:** an array of attribute name strings, in document order.
Alias: `attr_keys`.

**Errors:** raises `InvalidType` if the cursor isn't a body/block.

```lua
for _, k in h:block("resource", "aws_instance", "web"):attribute_keys() do
  print(k)
end
```

## `h:blocks([ident[, label...]])`

Handles to matching nested blocks.

**Signature:** `h:blocks() -> {HclDoc}`, `h:blocks(ident: string, ...) -> {HclDoc}`

**Parameters:**
- *(none)* — every nested block, any identifier/labels.
- `ident` plus an optional label pattern (same shapes as `:block`, minus
  a trailing `nth` — every match is returned, each already carrying its
  own resolved `nth`).

**Returns:** an array of `HclDoc` cursors, each pointing at one matching
block (its `nth` set so it round-trips through further navigation).

**Errors:** raises `InvalidType` if the cursor isn't a body/block.

```lua
for _, r in h:blocks("resource", "aws_instance") do
  print(r:attr("ami"):read())
end
```

## `h:block_types()`

The distinct block identifiers present in a body or block.

**Signature:** `h:block_types() -> {string}`

**Returns:** an array of identifier strings, first-seen order, each
appearing once regardless of how many blocks share it.

**Errors:** raises `InvalidType` if the cursor isn't a body/block.

```lua
for _, t in h:block_types() do print(t) end   -- e.g. "resource", "variable"
```

## `h:block_labels(ident)`

The distinct label tuples used by blocks named `ident`.

**Signature:** `h:block_labels(ident: string) -> {{string}}`

**Parameters:**
- `ident` — block type identifier.

**Returns:** an array of label arrays (one per distinct label tuple,
first-seen order); a block with zero labels contributes `{}`.

**Errors:** raises `InvalidType` if the cursor isn't a body/block.

```lua
for _, labels in h:block_labels("resource") do
  print(table.concat(labels, "."))  -- e.g. "aws_instance.web"
end
```

## `h:block_count(ident[, label...])`

Count of blocks matching `ident` and a label pattern.

**Signature:** `h:block_count(ident: string, ...) -> integer`

**Parameters:**
- `ident` — block type identifier (required).
- optional label pattern, same shapes as `:blocks`' tail (omitted =
  match any labels).

**Returns:** the number of matching blocks.

**Errors:** raises `InvalidType` if the cursor isn't a body/block; a
missing/non-string `ident` raises immediately.

```lua
if h:block_count("resource", "aws_instance", "**") > 1 then ... end
```

## `h:is_array()` / `h:is_object()` / `h:is_expr()` / `h:is_block()` / `h:is_body()`

Type checks on what the cursor currently resolves to.

**Signature:** `h:is_array() -> boolean` (etc., all zero-arg, all
returning `boolean`)

**Returns:** `false` if the path doesn't resolve, or resolves to a
different kind. `is_expr()` is also `true` for object/array targets
(anything read-capable as an expression). `is_body()` is `true` for an
empty (root) path. None of these raise.

```lua
local cur = h:attr("tags")
if cur:is_object() then
  for _, m in cur:attributes() do print(m:read()) end
end
```

## `h:to_string()`

Render the (sub)document at the cursor back to HCL text.

**Signature:** `h:to_string() -> string`

**Returns:** for the root (empty path), the whole document; for a
`Block` target, that block alone; for `Object`/`Array`/`Expr`, the
expression's own textual form. Also reachable via Luau's `tostring(h)`.

**Errors:** raises if the path doesn't resolve.

```lua
fs.write("main.tf", h:to_string())
print(tostring(h:block("resource", "aws_instance", "web")))
```

## `try_*` variants

Every fallible `HclDoc` method above has a `try_` counterpart:
`try_read`, `try_read_raw`, `try_write`, `try_add`, `try_remove`,
`try_len`, `try_attributes`/`try_attrs`, `try_attribute_keys`/
`try_attr_keys`, `try_blocks`, `try_block_types`, `try_block_labels`,
`try_block_count`, `try_to_string`.

**Signature:** same parameters as the non-`try_` form.

**Returns:** read-like calls (`try_read`, `try_read_raw`,
`try_attributes`, `try_attribute_keys`, `try_blocks`, `try_block_types`,
`try_block_labels`, `try_block_count`, `try_len`, `try_to_string`)
return the normal value on success, `nil` on any error. Write-like calls
(`try_write`, `try_add`, `try_remove`) return `true`/`false`.

**Errors:** never raise — that's the point.

```lua
local ami = h:block("resource", "aws_instance", "missing"):attr("ami"):try_read()
if ami == nil then print("no such attribute") end
```
