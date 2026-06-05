# `hcl` — HCL

| Function | Effect |
| --- | --- |
| `hcl::parse(text)` | Parse HCL into native Rhai values (blocks nest by label). |
| `hcl::to_string(value)` | Serialize a Rhai value to HCL. |
| `hcl::builder()` | Start building an HCL document (a body). |
| `hcl::block(ident)` | Start building an HCL block. |
| `hcl::expr(text)` | A raw HCL expression value (emitted unquoted). |
| `hcl::ident(name)` | A bare identifier/variable reference. |
| `hcl::edit(text)` | Parse HCL into an editable, format-preserving handle. |

`parse` / `to_string` mirror `json` and `toml`. As with `toml`, HCL does
not necessarily round-trip.

The builder is fluent. `HclBody` and `HclBlock` both expose chainable
`.with_attribute(key, value)`, `.with_block(child)`, and
`.with_comment(text)`; blocks additionally have `.with_label(text)`.
These are `with_*` methods: each mutates the builder and returns it for
chaining (see [Method naming conventions](../conventions.md)).
`body.to_string()` renders the document.

```rhai
let inner = hcl::block("tags").with_attribute("Env", "prod");
let res   = hcl::block("resource").with_label("aws_instance").with_label("web")
              .with_attribute("ami", "ami-123")
              .with_block(inner);
let doc   = hcl::builder()
              .with_attribute("region", "us-east-1")
              .with_block(res);
fs::write("/main.tf", doc.to_string());
```

Attribute values accept any Rhai value (scalars, arrays, maps) and are
converted to HCL expressions. A plain string is emitted *quoted*. To
emit a bare keyword, traversal, or function call instead, wrap it with
`hcl::expr` (a full HCL expression) or `hcl::ident` (a single validated
identifier):

```rhai
let v = hcl::block("variable").with_label("infra_env")
          .with_attribute("type", hcl::expr("string"))   // type = string
          .with_attribute("default", "dev");              // default = "dev"
fs::write("/variables.tf", hcl::builder().with_block(v).to_string());
```

`hcl::expr` parses arbitrary expressions (`var.region`, `max(1, 2)`,
`["a", "b"]`); `hcl::ident("foo")` is the bare-identifier shorthand.
Invalid input is a runtime error. These are builder values only — they
do not apply to the `hcl::to_string(value)` serializer.

## Comments

`.with_comment(text)` attaches a leading comment. You supply the comment
marker(s): every line of `text` must start with `#` or `//` (multi-line
is fine if *each* line is a valid comment line); anything else is a
runtime error. The marker is never added for you.

Placement depends on builder state:

- on a body with **no** items yet → a header comment at the top of the
  document;
- on a block with **no** items yet → comments the block's own header;
- otherwise → comments the most-recently-added item.

```rhai
let v = hcl::block("variable").with_label("aws_account_id")
          .with_attribute("type", hcl::expr("string"));
let doc = hcl::builder()
            .with_block(v)
            .with_comment("# tflint-ignore: terraform_unused_declarations");
fs::write("/variables.tf", doc.to_string());
```

```hcl
# tflint-ignore: terraform_unused_declarations
variable "aws_account_id" {
  type = string
}
```

Comments are leading-only (no trailing comments). The builder generates
code; comments are not produced by `hcl::parse`/`hcl::to_string`.

## Editing

`hcl::edit(<text>)` parses an existing HCL document into an editable
handle that **preserves layout and comments**. Unlike the lossy
`hcl::parse`/`hcl::to_string` round-trip, edits are surgical: untouched
lines (including comments and blank-line spacing) survive verbatim. Pair
it with `fs::write(<path>, …)` to write the result back.

```rhai
let h = hcl::edit(fs::read("/main.tf"));
h.block("resource", "aws_instance", "web").attr("ami").write("ami-123");
fs::write("/main.tf", h.to_string());
```

The handle is a *cursor*: traversal calls build up a path and return a new
handle (the original is unchanged), while read/write/add/remove resolve
that path against the shared document.

A cursor can land on an **attribute value** (an expression — readable,
writable), an **array** or **object** (indexable, addable), a **block**
(navigable, but not a readable/writable value — use it to reach
attributes or nested blocks, or to `.add`/`.write`/`.remove` whole
blocks), or the **body** (the document root). `is_block()`/`is_body()`,
`is_expr()`/`is_array()`/`is_object()` report which.

### Traversal

| Call | Navigates to |
| --- | --- |
| `h[<key>]` | An attribute, or a **label-less** block, named `<key>`. (A block *with* labels needs `.block(<ident>, <label>…)`.) |
| `h[<index>]` | The element at `<index>` of an array. |
| `h.attr(<key>)` | An attribute named `<key>` (disambiguates against a block). |
| `h.block(<ident>[, <label>…][, <nth>])` | A block by identifier, optional labels, and optional `<nth>` index when several match. |

`h.block(<ident>, <labels>)` also accepts an array of labels. A block
reference is ambiguous (runtime error) if several match and no `<nth>`
is given.

#### Matching labels

The label arguments to `block`, `blocks`, and `block_count` are a
*pattern*, matched against a block's labels position-by-position:

- a literal string matches that exact label;
- `"*"` matches exactly **one** label, any value;
- `"**"` matches **zero or more** labels, any values.

So `["aws_instance", "*"]` matches any two-label block whose first label
is `aws_instance`; `["aws_instance", "**"]` matches `aws_instance`
followed by any number of further labels.

Omitting the labels argument entirely (`block("resource")`,
`block_count("resource")`, `blocks("resource")`) matches a block with
**any** labels — equivalent to `["**"]`. To match only a block with
*exactly zero* labels, pass an explicit empty array `[]`. The pattern is
anchored at both ends: without a trailing `"**"`, its length must equal
the block's label count.

### Reading and inspecting

| Call | Returns |
| --- | --- |
| `.read()` | The value at the path as a native Rhai value (literals only). |
| `.read_raw()` | The expression at the path verbatim, as a raw value (any expression). |
| `.exists()` | Whether the path resolves. |
| `.len()` | Item count of a body/block/object/array. |
| `.attributes()` / `.attrs()` | Handles to each attribute. |
| `.attribute_keys()` / `.attr_keys()` | The attribute names. |
| `.blocks([<ident>[, <label>…]])` | Handles to matching blocks (labels are a pattern — see above). |
| `.block_types()` | The distinct block identifiers. |
| `.block_labels(<ident>)` | The distinct label tuples for `<ident>`. |
| `.block_count(<ident>[, <label>…])` | Count of blocks matching the label pattern (see above). |
| `.is_array()` / `.is_object()` / `.is_expr()` / `.is_block()` / `.is_body()` | Type checks. |

### Mutating

| Call | Effect |
| --- | --- |
| `.write(<value>)` | Replace the value at the path. |
| `.write(<block>)` | Replace the block the cursor points at with an `hcl::block(…)` builder. |
| `.add(<key>, <value>)` | Add an attribute to a body/block/object. |
| `.add(<block>)` | Append an `hcl::block(…)` builder to a body/block. |
| `.add(<value>)` | Append to an array. |
| `.add(<index>, <value>)` | Insert into an array at `<index>`. |
| `.remove()` | Remove the element at the path. |
| `.remove(<key>)` | Remove a child attribute/block by key. |
| `.remove(<index>)` | Remove an array element by index. |

Attributes and blocks added to the document are indented to match their
surroundings (the indent of an existing sibling, or two spaces per
nesting level for an empty body).

Whole blocks are created with the same fluent `hcl::block(…)` builder
used for generating documents, then spliced into the editable handle.
`add` appends; `write` replaces the block at the cursor, keeping its
position:

```rhai
let h = hcl::edit(fs::read("/main.tf"));
// Append a new block to the document.
h.add(hcl::block("output").with_label("id").with_attribute("value", hcl::expr("aws_instance.web.id")));
// Replace an existing block wholesale.
h.block("resource", "aws_instance", "web")
  .write(hcl::block("resource").with_label("aws_instance").with_label("web").with_attribute("ami", "ami-456"));
fs::write("/main.tf", h.to_string());
```

Finally, `.to_string()` renders the (sub)document back to HCL text.

### `try_` variants and value support

Every fallible call has a `try_` sibling that swallows the error: read-like
`try_*` return `()` on failure (e.g. `h["nope"].try_read()` is `()`),
write-like `try_*` return `false`. Use them when a path may be absent.

Only **literal** expressions `.read()` back as native Rhai values: strings,
numbers, bools, `null`, arrays, and objects. Computed expressions —
`var.region`, function calls, conditionals, templates — cannot be `.read()`
as values (that errors / `try_read()` yields `()`), but they are left
**untouched** and survive `.to_string()`.

To *write* a computed expression rather than a literal, pass a builder
`hcl::expr(<text>)` (any HCL expression) or `hcl::ident(<name>)` (a bare
identifier) to `.write(…)` / `.add(…)`; it is emitted verbatim, unquoted:

```rhai
let h = hcl::edit(fs::read("/variables.tf"));
h.block("variable", "x").attr("type").write(hcl::expr("list(string)"));
// type = list(string)
```

`.read_raw()` is the dual of that write path: it reads **any** expression
(literal or computed) verbatim, returning the same kind of raw value
`hcl::expr` produces. That value round-trips straight back into `.write(…)`
/ `.add(…)` (and the builder), so you can copy an expression from one place
to another without inspecting it. `.try_read_raw()` yields `()` on a
non-expression path instead of erroring.

```rhai
let h = hcl::edit(fs::read("/variables.tf"));
let t = h.block("variable", "src").attr("type").read_raw();
h.block("variable", "dst").add("type", t);   // copy the type verbatim
```
