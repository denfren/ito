# `hcl` — HCL

| Function | Effect |
| --- | --- |
| `hcl.parse(text, [name])` | Parse HCL into a native Luau value (lossy). |
| `hcl.to_string(value)` | Serialize a Luau value to HCL text (lossy). |
| `hcl.builder()` | Start building an HCL document (a body). |
| `hcl.block(ident)` | Start building an HCL block. |
| `hcl.expr(text)` | Parse a raw HCL expression, emitted verbatim/unquoted. |
| `hcl.ident(name)` | A bare identifier/variable-reference expression. |
| `hcl.edit(text, [name])` | Parse HCL into an editable, format-preserving handle. |

| Builder method | Effect |
| --- | --- |
| `body:with_attribute(key, value)` | Append an attribute; returns `body`. |
| `body:with_block(block)` | Append a nested block; returns `body`. |
| `body:with_comment(text)` | Attach a leading comment; returns `body`. |
| `body:to_string()` / `block:to_string()` | Render the body/block to HCL text. |
| `block:with_label(text)` | Append a quoted label; returns `block`. |
| `block:with_attribute(key, value)` | Append an attribute; returns `block`. |
| `block:with_block(child)` | Append a nested block; returns `block`. |
| `block:with_comment(text)` | Attach a leading comment; returns `block`. |
| `block:to_string()` | Render the block to HCL text. |

| `HclDoc` (from `hcl.edit`) | Effect |
| --- | --- |
| `h[key]` / `h[index]` | Navigate to an attribute/label-less block, or an array element. |
| `h:attr(key)` / `h:attribute(key)` | Navigate to an attribute, disambiguating against a block. |
| `h:block(ident[, label...][, nth])` | Navigate to a block by identifier/labels/occurrence. |
| `h:exists()` | Whether the cursor's path resolves. |
| `h:kind()` | The resolved node's kind, as a string. |
| `h:read()` | Read the value at the path as a native Luau value (literals only). |
| `h:raw()` | Read the expression at the path verbatim (any expression). |
| `h:write(value)` | Replace the value (or block) at the path. |
| `h:add(value)` / `h:add(key, value)` | Append an attribute/block/array element. |
| `h:add_at(index, value)` / `h:add_at(index, key, value)` | Insert an attribute/array element at a position. |
| `h:update(update_fn)` / `h:update(key, update_fn)` / `h:update(index, update_fn)` | Read-modify-write an attribute value in place. |
| `h:update_at(index, update_fn)` | Read-modify-write an attribute/array element by position. |
| `h:update_all(update_fn)` | Read-modify-write every attribute value of a body/block. |
| `h:upsert(key, upsert_fn)` | Update a keyed attribute, or add it if missing. |
| `h:upsert_at(index, key, upsert_fn)` | As `upsert`, inserting a new attribute at a position. |
| `h:remove()` / `h:remove(key)` / `h:remove(index)` | Remove the element at the path, or a named/indexed child. |
| `h:remove_at(index)` | Remove an array element, or a body/block attribute, by position. |
| `h:len()` | Item count of a body/block/object/array. |
| `h:attributes()` / `h:attrs()` | Handles to each attribute of a body/block, or each entry of an object. |
| `h:attribute_keys()` / `h:attr_keys()` | The attribute names of a body/block, or the keys of an object. |
| `h:blocks([ident[, label...]])` | Handles to matching nested blocks (all blocks without arguments). |
| `h:block_types()` | The distinct block identifiers in a body/block. |
| `h:block_labels(ident)` | The distinct label tuples for blocks named `ident`. |
| `h:block_count(ident[, label...])` | Count of blocks matching a label pattern. |
| `h:is_array()` / `h:is_object()` / `h:is_expr()` / `h:is_block()` / `h:is_body()` | Type checks on the cursor's target. |
| `h:to_string()` | Render the (sub)document back to HCL text. |
| `h:try_*(...)` | Non-raising sibling of every call above. |

## Example

```lua
-- Bump an AMI id in place, keeping every comment and blank line intact.
local h = hcl.edit(fs.read("main.tf"))
h:block("resource", "aws_instance", "web"):attr("ami"):write("ami-0123456789")
fs.write("main.tf", h:to_string())
```

## Conventions

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
  aliases `a` rather than copying it. A bad argument (an invalid attribute
  name, a non-string label, a comment line with no `#`/`//`) raises at that
  setter; `:to_string()` is the terminal and cannot fail for a builder that
  was assembled successfully.
- **Attribute values.** Any Luau value (scalars, tables/arrays) passed
  to `:with_attribute`, `:write`, or `:add` is converted like `json` does
  (global `null` is HCL `null`; an empty table is an object, use
  `table.array()` for an empty list): a plain string is emitted
  **quoted**. To emit a bare keyword, traversal, function call, or other
  computed expression, wrap it with `hcl.expr` or `hcl.ident` first —
  those pass through **verbatim**.
- **Keys are never rewritten.** An object key that is not an identifier
  (`"a.b"`) is emitted and looked up as a quoted string. An *attribute*
  name must be an identifier; anything else raises
  (`invalid attribute name`). `a.b` never turns into `a_b`.
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
  unchanged. `:raw()` reads *any* expression (literal or computed)
  as an opaque value that round-trips into `:write`/`:add`. The `update*`
  callbacks receive such a computed value as that raw handle, so
  `update_all` never fails on one: return it unchanged to keep it.
- **Every fallible `HclDoc` method has a `try_` sibling.** Read-like
  `try_*` return `nil` on failure; write-like `try_*` return `false`.
  Use them when a path may legitimately be absent. They swallow path and
  structure errors only: an error raised by *your own callback*
  (`try_update`, `try_upsert`, ...) propagates.
- **Removal is scoped.** `h:remove(key)` removes the attribute `key`, or
  the single *unlabelled* block of that name; both existing raises
  (`ambiguous key`). Labelled blocks are never removed by name alone:
  address them with `h:block(ident, label...):remove()`.
- **A path is a coordinate.** Handles from `:blocks()` carry an occurrence
  number, so removing a block shifts the handles after it (as for yaml
  sequences). A literal `*` or `**` label in a pattern is always a
  wildcard; there is no way to match those characters literally.
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

## `hcl.parse`

Parse HCL text into a native Luau value (lossy — like `json.parse`).

**Signature:** `hcl.parse(text: string, name: string?) -> value`

**Parameters:**
- `text` — HCL source text.
- `name` — optional, added to parse errors.

**Returns:** a Luau value: tables for objects/blocks and arrays,
strings, numbers, booleans, and the global `null` per the usual
JSON-ish mapping. Blocks with labels nest under their label path.

**Errors:** raises `hcl parse error [in <name>]: ...` on malformed HCL.

```lua
local doc = hcl.parse(fs.read("main.tf"))
print(doc.resource.aws_instance.web.ami)
```

## `hcl.to_string`

Serialize a Luau value to HCL text (lossy — like `toml.to_string`; does
not necessarily round-trip with `hcl.parse`).

**Signature:** `hcl.to_string(value: any) -> string`

**Parameters:**
- `value` — any Luau value `json.to_string` accepts (tables, strings,
  numbers, bools, `null`). A top-level key must be an identifier.

**Returns:** the HCL text.

**Errors:** raises `"hcl serialize error: ..."` if the value can't be
represented (e.g. unsupported nesting).

```lua
local text = hcl.to_string({ region = "us-east-1" })
```

## `hcl.builder`

Start building an HCL document (the top-level body).

**Signature:** `hcl.builder() -> body`

**Returns:** a fresh `body` handle with no attributes or blocks.

**Methods:** `with_attribute` · `with_block` · `with_comment` · `to_string`

```lua
local doc = hcl.builder():with_attribute("region", "us-east-1")
fs.write("main.tf", doc:to_string())
```

## `hcl.block`

Start building an HCL block.

**Signature:** `hcl.block(ident: string) -> block`

**Parameters:**
- `ident` — the block type identifier (e.g. `"resource"`, `"variable"`).
  Validated as an HCL identifier, but only at render time (`:to_string()`
  / splicing into `hcl.edit`) — `hcl.block("")` itself does not raise.

**Returns:** a fresh `block` handle with no labels, attributes, or
blocks.

**Methods:** `with_label` · `with_attribute` · `with_block` · `with_comment` · `to_string`

**Errors:** rendering (`:to_string()`, or splicing via `:write`/`:add`
into an `hcl.edit` handle) raises `"hcl identifier error: ..."` if
`ident` is not a valid HCL identifier.

```lua
local b = hcl.block("resource"):with_label("aws_instance"):with_label("web")
```

## `body:with_attribute` / `block:with_attribute`

Append an attribute (`key = value`) to a body or block.

**Signature:** `body:with_attribute(key: string, value: any) -> body`
(same shape on `block`, returning `block`)

**Parameters:**
- `key` — the attribute name; must be an HCL identifier.
- `value` — a plain Luau value (converted like `json`, so a string is
  emitted quoted) or an `HclExpr` from `hcl.expr`/`hcl.ident` (emitted
  verbatim).

**Returns:** the receiver (chainable).

**Errors:** raises immediately `hcl identifier error: ...` for an invalid
`key`, `hcl builder error: duplicate attribute ...` if the body/block
already has an attribute `key`, and `hcl write error ...` if `value` can't
convert.

```lua
local v = hcl.block("variable"):with_label("infra_env")
  :with_attribute("type", hcl.expr("string"))   -- type = string
  :with_attribute("default", "dev")              -- default = "dev"
```

## `body:with_block` / `block:with_block`

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

## `body:with_comment` / `block:with_comment`

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

## `block:with_label`

Append a label to a block header.

**Signature:** `block:with_label(text: string) -> block`

**Parameters:**
- `text` — the label value; rendered quoted with HCL's own escaping
  (including `${` and `%{`), so it needs no pre-quoting.

**Returns:** `block` (chainable).

```lua
local b = hcl.block("resource"):with_label("aws_instance"):with_label("web")
-- resource "aws_instance" "web" { ... }
```

## `body:to_string` / `block:to_string`

Render a built body or block to HCL text. Both handles also answer
`tostring(handle)` the same way.

**Signature:** `body:to_string() -> string`, `block:to_string() -> string`

**Returns:** for a body, the rendered document (attributes and blocks in
insertion order; a blank line is inserted around every block). For a
block, that block's own text, without a surrounding document.

**Errors:** raises on an invalid identifier or unconvertible value
introduced by an earlier `with_*` call (validation is deferred to
render time).

```lua
fs.write("main.tf", hcl.builder():with_attribute("region", "us-east-1"):to_string())
print(hcl.block("output"):with_label("id"):to_string())
```

A built block can also be spliced directly into an `hcl.edit` handle via
`:write`/`:add`.

## `hcl.expr`

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

## `hcl.ident`

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

## `hcl.edit`

Parse HCL into an editable, format-preserving handle.

**Signature:** `hcl.edit(text: string) -> HclDoc`

**Parameters:**
- `text` — HCL source text.

**Returns:** an `HclDoc` cursor rooted at the document body (an empty
navigation path — `:is_body()` is `true`).

**Errors:** raises `hcl parse error [in <name>]: ...` on malformed HCL.
The optional second argument `name` (`hcl.edit(text, name)`) is added to
the message.

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

## `h:attr` / `h:attribute`

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

## `h:block`

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

## `h:exists`

Whether the cursor's path resolves against the current document.

**Signature:** `h:exists() -> boolean`

**Returns:** `true`/`false`; never raises (a root cursor with an empty
path is always `true`).

```lua
if h:block("provider", "aws"):exists() then ... end
```

## `h:read`

Read the value at the path as a native Luau value.

**Signature:** `h:read() -> value`

**Returns:** the same value shapes `hcl.parse` produces (strings,
numbers, bools, `null`, tables) for a **literal** attribute value,
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

## `h:raw`

Read the expression at the path **verbatim**, as an opaque value — the
same kind `hcl.expr`/`hcl.ident` produce. Unlike `:read()`, this works
for computed expressions (`var.x`, function calls, `list(string)`,
conditionals) and round-trips straight back into `:write`/`:add`.

**Signature:** `h:raw() -> HclExpr`

**Returns:** an opaque `HclExpr`. The cursor must point at an
expression (attribute value / array element / object value), not a
body or block.

**Errors:** raises `InvalidType` if the cursor targets a body or block.

```lua
local t = h:block("variable", "src"):attr("type"):raw()
h:block("variable", "dst"):add("type", t)   -- copy the type verbatim
```

## `h:kind`

The resolved node's kind, as a string.

**Signature:** `h:kind() -> string`

**Returns:** one of `"body"`, `"block"`, `"object"`, `"array"`, `"expr"`
— the same value the `is_*` predicates check against, so `h:kind() ==
"block"` and `h:is_block()` can never disagree.

**Errors:** raises if the path doesn't resolve (see `:block`/`h[key]`
above). `:try_kind()` returns `nil` instead.

```lua
print(h:block("resource", "aws_instance", "web"):kind())  -- "block"
```

## `h:write`

Replace the value at the path.

**Signature:** `h:write(value: any) -> ()`

**Parameters:**
- `value` — a plain Luau value (converted like `json`; strings are
  quoted), an `HclExpr` (emitted verbatim), or — only when the cursor
  points at a **block** — a `block` builder from `hcl.block(...)`, which
  replaces the whole block in place, keeping its position and
  indentation. The replaced block's leading comments and blank lines are
  kept, unless the builder carries its own `with_comment`, which wins.

**Returns:** nothing.

**Errors:** raises `not found: <segment> (at <path>)` if the target
key/index doesn't exist in the parent; `expected a body, block, object
or array, but this is a ...` if the parent has the wrong shape (or, for
the block-replace form, if the cursor's last path segment isn't a
block); `index N out of range ...` for an out-of-range array index.

```lua
h:block("resource", "aws_instance", "web"):attr("ami"):write("ami-0123456789")

-- replace a whole block, keeping its position
h:block("resource", "aws_instance", "web")
  :write(hcl.block("resource"):with_label("aws_instance"):with_label("web")
           :with_attribute("ami", "ami-456"))
```

## `h:add` / `h:add`

Append an attribute, a nested block, or an array element.

**Signature:** `h:add(value)`, `h:add(key: string, value)`

**Parameters:** (dispatch by arity/type):
- `h:add(value)` — the cursor must target a body/block (appends a
  nested `block` builder) or an array (appends `value`, converted like
  `:write`).
- `h:add(key: string, value)` — the cursor must target a body/block/
  object; adds an attribute/member named `key`, appended after the
  existing ones. Errors if `key` already exists.

**Returns:** nothing.

**Errors:** raises `InvalidType` if the cursor's target doesn't match
the call shape; raises `"duplicate key: ..."` for an existing `key`;
raises for a wrong argument count (`"add: expected 1 or 2 arguments,
got N"`) or a first argument that's neither an integer nor a string in
the 2-arg form.

Added attributes/blocks are indented to match an existing sibling, or
two spaces per nesting level if the body is empty. A block added to a
non-empty body is preceded by a blank line.

```lua
-- append an attribute
h:block("resource", "aws_instance", "web"):add("tags", { Env = "prod" })

-- append a new block to the document
h:add(hcl.block("output"):with_label("id")
        :with_attribute("value", hcl.expr("aws_instance.web.id")))

-- array append
h:attr("ports"):add(8080)
```

## `h:add_at` / `h:add_at`

Insert an attribute or an array element at a given position, rather
than always appending.

**Signature:** `h:add_at(index: integer, value)`, `h:add_at(index:
integer, key: string, value)`

**Parameters:**
- `index` — 1-based position. For an array: the element position;
  out-of-range (including `<= 0`) clamps to the nearest end rather than
  erroring. For a body/block/object: the position among **attributes
  only** (nested blocks don't count), matching what `:attribute_keys()`
  enumerates; an index past the end clamps to append.
- `key` (body/block/object form only) — the new attribute's name.
  Errors if `key` already exists.
- `value` — as for `:add`.

**Returns:** nothing.

**Errors:** raises `InvalidType` if the cursor's target doesn't match
the call shape; raises `"duplicate key: ..."` for an existing `key`;
raises for a wrong argument count or a non-integer first argument.

```lua
-- insert an attribute before the first existing one
h:block("variable", "x"):add_at(1, "default", "dev")

-- array insert at position 1
h:attr("ports"):add_at(1, 22)
```

## `h:update` / `h:update` / `h:update`

Read-modify-write a single attribute or array element in place.

**Signature:** `h:update(update_fn: (value) -> value)`, `h:update(key:
string, update_fn)`, `h:update(index: integer, update_fn)`

**Parameters:**
- *(no key/index)* — self-update: the cursor itself must point at an
  attribute value.
- `key: string` — updates the named attribute of a body/block.
- `index: integer` — updates an array element by position, **or** a
  body/block attribute by position (whichever the cursor resolves to);
  `:update_at(index, update_fn)` is the unambiguous spelling for the
  by-position form on either kind.
- `update_fn` — called with the attribute/element's current decoded
  value; a computed expression (`var.x`, a call, ...) is passed as a raw
  handle (as `:raw()` returns) that writes back verbatim. Its return
  value is written back, or the attribute/element is **removed** if it
  returns `nil`.

**Returns:** nothing.

**Errors:** raises the same errors `:read()`/`:write()`/`:remove()`
would for the resolved target (wrong shape, not found, index out of
range); raises for a wrong argument count or a first argument that's
neither an integer, a string, nor omitted. An error raised inside
`update_fn` propagates (also through `:try_update`).

```lua
h:block("resource", "aws_instance", "web"):attr("ami"):update(function(v)
  return v .. "-v2"
end)

h:block("resource", "aws_instance", "web"):update("ami", function(v)
  return v .. "-v2"
end)
```

## `h:update_at`

Read-modify-write an array element, or a body/block attribute, by
1-based position — the explicit form of `:update(index, update_fn)`.

**Signature:** `h:update_at(index: integer, update_fn: (value) -> value)`

**Parameters:**
- `index` — 1-based array element position, or attribute position
  (counting only attributes, as for `:add_at`).
- `update_fn` — as for `:update`.

**Returns:** nothing.

**Errors:** as for `:update`.

```lua
h:block("variable", "x"):update_at(1, function(v) return v .. "-x" end)
```

## `h:update_all`

Read-modify-write every attribute value of a body/block, in place.

**Signature:** `h:update_all(update_fn: (value) -> value)`

**Parameters:**
- `update_fn` — called once per attribute, with its current decoded
  value (a computed expression arrives as a raw handle; return it to keep
  it); its return value is written back, or that attribute is removed
  if it returns `nil`. Nested blocks are skipped — only attribute
  values participate.

**Returns:** nothing. The operation is **atomic**: every callback runs
before anything is changed, so one that raises leaves the document
untouched.

**Errors:** raises if the cursor isn't a body/block; an error from
`update_fn` propagates.

```lua
h:block("resource", "aws_instance", "web"):update_all(function(v)
  if type(v) == "string" then return v .. "-tagged" end
  return v
end)
```

## `h:upsert`

Update a keyed attribute if present, or add it if missing — the
non-raising sibling of `:update(key, ...)` for the "might not exist"
case.

**Signature:** `h:upsert(key: string, upsert_fn: (value | nil) -> value | nil)`

**Parameters:**
- `key` — the attribute name.
- `upsert_fn` — if `key` exists, called with its current decoded value
  (as for `:update`); if `key` is missing, called with `nil`. Its
  return value is written back (existing key) or **appended** as a new
  attribute (missing key) — unless it returns `nil`, which removes an
  existing attribute or, for a missing key, adds nothing.

**Returns:** nothing.

**Errors:** raises `InvalidType` if the cursor isn't a body/block;
raises `UnsupportedHclType` if an existing value isn't literal-readable.

```lua
h:block("resource", "aws_instance", "web"):upsert("region", function(v)
  if v == nil then return "eu-west-1" end
  return v
end)
```

## `h:upsert_at`

As `:upsert`, but a newly-added attribute is inserted at a given
position instead of appended.

**Signature:** `h:upsert_at(index: integer, key: string, upsert_fn:
(value | nil) -> value | nil)`

**Parameters:**
- `index` — 1-based attribute position (as for `:add_at`), used only
  when `key` is missing and `upsert_fn` returns non-`nil`.
- `key`, `upsert_fn` — as for `:upsert`.

**Returns:** nothing.

**Errors:** as for `:upsert`.

```lua
h:block("resource", "aws_instance", "web"):upsert_at(1, "region", function(v)
  if v == nil then return "eu-west-1" end
  return v
end)
```

## `h:remove` / `h:remove` / `h:remove`

Remove the element at the cursor's own path, or a named/indexed child.

**Signature:** `h:remove()`, `h:remove(key)`, `h:remove(index)`

**Parameters:**
- *(none)* — removes the element the cursor itself points at (resolved
  against its **parent**); works for an attribute, a block (matched by
  the cursor's own ident/labels/`nth`), an object member, or an array
  element.
- `key: string` — removes the child attribute `key`, or the single
  *unlabelled* block named `key`, from the body/block/object the cursor
  points at. Labelled blocks are never removed this way. If both an
  attribute and an unlabelled block have that name, it raises
  (`ambiguous key`).
- `index: integer` — removes the 1-based element from the array the
  cursor points at.

**Returns:** nothing.

**Errors:** raises `not found` if there's nothing to remove; a
wrong-shape error if the cursor/parent doesn't match; `index out of
range` for a bad array index; a bad argument type/count raises
immediately.

```lua
h:block("resource", "aws_instance", "old"):remove()   -- remove the block itself
h:block("resource", "aws_instance", "web"):remove("ami")  -- remove one attribute
h:attr("ports"):remove(1)                              -- remove array element 1
```

## `h:remove_at`

Remove an array element, or a body/block attribute, by 1-based
position — kept as a distinct spelling from `:remove(key)` rather than
guessed from argument type, and extends the position concept `:remove`
already had for arrays to body/block attributes too.

**Signature:** `h:remove_at(index: integer)`

**Parameters:**
- `index` — 1-based array element position, or attribute position
  (counting only attributes, as for `:add_at`).

**Returns:** nothing.

**Errors:** raises `InvalidType` if the cursor isn't an array, body, or
block; raises `IndexOutOfBounds` if `index` is out of range.

```lua
h:block("variable", "x"):remove_at(1)   -- remove the first attribute
h:attr("ports"):remove_at(1)             -- remove array element 1
```

## `h:len`

Item count of the cursor's target.

**Signature:** `h:len() -> integer`

**Returns:** attribute+block count for a body/block, member count for
an object, element count for an array.

**Errors:** raises `InvalidType` for an `Expr` (scalar) target.

```lua
print(h:block("locals"):len())
```

## `h:attributes` / `h:attrs`

Handles to every attribute in a body or block.

**Signature:** `h:attributes() -> {HclDoc}`

**Returns:** an array of `HclDoc` cursors, one per attribute (or one per
entry, for an object), in document order. Alias: `attrs`.

**Errors:** raises if the cursor isn't a body, block or object.

```lua
for _, a in h:attributes() do print(a:read()) end
```

## `h:attribute_keys` / `h:attr_keys`

The attribute names of a body or block.

**Signature:** `h:attribute_keys() -> {string}`

**Returns:** an array of attribute name strings (or object keys), in
document order. Alias: `attr_keys`.

**Errors:** raises if the cursor isn't a body, block or object.

```lua
for _, k in ipairs(h:block("resource", "aws_instance", "web"):attribute_keys()) do
  print(k)
end
```

## `h:blocks`

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
for _, r in ipairs(h:blocks("resource", "aws_instance")) do
  print(r:attr("ami"):read())
end
```

## `h:block_types`

The distinct block identifiers present in a body or block.

**Signature:** `h:block_types() -> {string}`

**Returns:** an array of identifier strings, first-seen order, each
appearing once regardless of how many blocks share it.

**Errors:** raises `InvalidType` if the cursor isn't a body/block.

```lua
for _, t in h:block_types() do print(t) end   -- e.g. "resource", "variable"
```

## `h:block_labels`

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

## `h:block_count`

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

## `h:is_array` / `h:is_object` / `h:is_expr` / `h:is_block` / `h:is_body`

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

## `h:to_string`

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
`try_read`, `try_raw`, `try_write`, `try_add`, `try_add_at`,
`try_update`, `try_update_at`, `try_update_all`, `try_upsert`,
`try_upsert_at`, `try_remove`, `try_remove_at`, `try_len`, `try_kind`,
`try_attributes`/`try_attrs`, `try_attribute_keys`/`try_attr_keys`,
`try_blocks`, `try_block_types`, `try_block_labels`, `try_block_count`,
`try_to_string`.

**Signature:** same parameters as the non-`try_` form.

**Returns:** read-like calls (`try_read`, `try_raw`, `try_kind`,
`try_attributes`, `try_attribute_keys`, `try_blocks`, `try_block_types`,
`try_block_labels`, `try_block_count`, `try_len`, `try_to_string`)
return the normal value on success, `nil` on any error. Write-like calls
(`try_write`, `try_add`, `try_add_at`, `try_update`, `try_update_at`,
`try_update_all`, `try_upsert`, `try_upsert_at`, `try_remove`,
`try_remove_at`) return `true`/`false`.

**Errors:** they raise only for an error raised by *your own callback*
(`try_update`, `try_update_all`, `try_upsert`, `try_upsert_at`): that error
is not a path/structure failure to swallow, so it propagates unchanged.

```lua
local ami = h:block("resource", "aws_instance", "missing"):attr("ami"):try_read()
if ami == nil then print("no such attribute") end
```
