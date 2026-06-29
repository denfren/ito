# `tablefmt` — render tables

Build a table from rows and render it as aligned text — pure computation, no
disk access. Named `tablefmt` rather than `table`, which is Luau's own
built-in library (see [`table`](table.md)); this module does not touch it.

| Function | Effect |
| --- | --- |
| `tablefmt.new(columns)` | Start a builder with the given column key names. |
| `t:headers(names)` | Set the pretty header text for every column. |
| `t:headers(nil)` | Omit the header row entirely. |
| `t:header(col, name)` | Set the pretty header text for one column. |
| `t:row(data)` | Add one row. |
| `t:rows(data)` | Add a list of rows. |
| `t:align(col, alignment)` | Align a whole column. |
| `t:align(row, col, alignment)` | Align a single cell. |
| `t:preset(name)` | Switch the border style. |
| `t:to_string([width])` | Render to a string. |
| `t:print([width])` | Render and print to stdout. |

```lua
local t = tablefmt.new({"name", "age"})
t:headers({"Name", "Age"})
t:rows({
    {name = "Ada", age = 36},
    {name = "Grace", age = 85},
})
t:align("age", tablefmt.RIGHT)
t:print()
-- ┌───────┬─────┐
-- │ Name  ┆ Age │
-- ╞═══════╪═════╡
-- │ Ada   ┆  36 │
-- ├╌╌╌╌╌╌╌┼╌╌╌╌╌┤
-- │ Grace ┆  85 │
-- └───────┴─────┘
```

## Conventions

- `tablefmt.new(columns)` takes the column **keys** — the names a row's data
  is looked up by, and the default header text until `:headers()` or
  `:header()` says otherwise.
- A row is either **keyed** by those column names (`{name = "Ada", age =
  36}`) or **positional** (`{"Ada", 36}`, matched against the declared
  column order) — both forms are accepted on the same table, since a script
  assembling rows from a query result and a script building them by hand
  want different shapes. A missing keyed field (and a missing positional
  slot) renders as an empty cell.
- Every builder method **returns the builder itself**, so calls chain:
  `tablefmt.new({"a"}):row({"x"}):print()`.
- Cell content is rendered with Lua's own `tostring`, so a pre-colored
  string from `("x"):red()` passes through into the cell verbatim, and
  column widths still line up correctly — width measurement is
  escape-aware.
- Alignment is one of the string constants `tablefmt.LEFT`,
  `tablefmt.CENTER`, `tablefmt.RIGHT`.
- Presets are the string constants `tablefmt.ASCII_FULL`,
  `tablefmt.ASCII_FULL_CONDENSED`, `tablefmt.ASCII_NO_BORDERS`,
  `tablefmt.ASCII_BORDERS_ONLY`, `tablefmt.ASCII_BORDERS_ONLY_CONDENSED`,
  `tablefmt.ASCII_HORIZONTAL_ONLY`, `tablefmt.ASCII_MARKDOWN`,
  `tablefmt.UTF8_FULL` (the default), `tablefmt.UTF8_FULL_CONDENSED`,
  `tablefmt.UTF8_NO_BORDERS`, `tablefmt.UTF8_BORDERS_ONLY`,
  `tablefmt.UTF8_HORIZONTAL_ONLY`, and `tablefmt.NOTHING` (no borders or
  padding at all).
- Columns are arranged dynamically to fit the given width, wrapping cell
  content as needed. Without a width, `:print()`/`:to_string()` fall back
  to the terminal's own width when stdout is a tty, or the content's
  natural width otherwise.

## `tablefmt.new`

Starts a new table builder.

**Signature:** `tablefmt.new(columns: {string}) -> Builder`

**Parameters:**
- `columns` — array of column key names, in display order. Also the
  default header text for each column.

**Returns:** a builder, ready for `:headers()`/`:row()`/.../`:print()`.

**Errors:** raises if `columns` is empty.

```lua
local t = tablefmt.new({"host", "status"})
```

## `Builder:headers`

Sets the pretty header text for every column at once.

**Signature:** `t:headers(names: {string} | nil) -> Builder`

**Parameters:**
- `names` — one header string per column, in the same order as
  `tablefmt.new`'s `columns`. Pass `nil` to omit the header row entirely —
  distinct from blank header text, this drops the row comfy-table would
  otherwise reserve for it.

**Returns:** `t`, for chaining.

**Errors:** raises if `names` is given and its length does not match the
column count.

```lua
tablefmt.new({"host", "status"}):headers({"Host", "Status"})
tablefmt.new({"host", "status"}):headers(nil) -- no header row at all
```

## `Builder:header`

Sets the pretty header text for a single column.

**Signature:** `t:header(col: number | string, name: string) -> Builder`

**Parameters:**
- `col` — the column's 1-based index, or its key name.
- `name` — the header text.

**Returns:** `t`, for chaining.

**Errors:** raises if `col` names no declared column, or if headers were
omitted via `:headers(nil)`.

```lua
local t = tablefmt.new({"host", "status"})
t:header("status", "State")
t:header(1, "Host name")
```

## `Builder:row`

Adds one row.

**Signature:** `t:row(data: table) -> Builder`

**Parameters:**
- `data` — a table keyed by column name, or a positional array, or a mix
  of both (a positional value is used only where the matching key is
  absent).

**Returns:** `t`, for chaining.

```lua
local t = tablefmt.new({"host", "status"})
t:row({host = "web-1", status = "ok"})
t:row({"web-2", "down"})
```

## `Builder:rows`

Adds a list of rows in one call.

**Signature:** `t:rows(data: {table}) -> Builder`

**Parameters:**
- `data` — array of row tables, each in the same shape `:row()` accepts.

**Returns:** `t`, for chaining.

```lua
tablefmt.new({"host", "status"}):rows({
    {host = "web-1", status = "ok"},
    {host = "web-2", status = "down"},
})
```

## `Builder:align`

Sets alignment for a whole column, or overrides it for a single cell.

**Signature:**
- `t:align(col: number | string, alignment: string) -> Builder`
- `t:align(row: number, col: number | string, alignment: string) -> Builder`

**Parameters:**
- `col` — the column's 1-based index, or its key name.
- `row` — the row's 1-based index (in the order added), for the per-cell
  form.
- `alignment` — `tablefmt.LEFT`, `tablefmt.CENTER`, or `tablefmt.RIGHT`.

**Returns:** `t`, for chaining.

**Errors:** raises if `col`/`row` is out of range, or `alignment` is not
one of the three constants.

```lua
local t = tablefmt.new({"name", "age"})
t:rows({{name = "Ada", age = 36}, {name = "Grace", age = 85}})
t:align("age", tablefmt.RIGHT)       -- every row's age, right-aligned
t:align(1, "name", tablefmt.CENTER)  -- just the first row's name
```

## `Builder:preset`

Switches the border/padding style.

**Signature:** `t:preset(name: string) -> Builder`

**Parameters:**
- `name` — one of the preset constants listed under "Conventions" above
  (e.g. `tablefmt.ASCII_FULL`). The default, before any `:preset()` call,
  is `tablefmt.UTF8_FULL`.

**Returns:** `t`, for chaining.

**Errors:** raises if `name` is not a known preset.

```lua
tablefmt.new({"a", "b"}):preset(tablefmt.ASCII_MARKDOWN):print()
-- | a | b |
-- |---|---|
```

## `Builder:to_string`

Renders the table to a string.

**Signature:** `t:to_string(width: number?) -> string`

**Parameters:**
- `width` — target total width in characters. Omit it to fall back to the
  terminal's width (when stdout is a tty) or the content's natural width.

**Returns:** the rendered table.

```lua
local rendered = tablefmt.new({"a"}):row({"x"}):to_string(20)
```

## `Builder:print`

Renders the table and writes it to stdout, followed by a newline.

**Signature:** `t:print(width: number?)`

**Parameters:**
- `width` — same as `:to_string()`.

**Returns:** nothing.

```lua
tablefmt.new({"a"}):row({"x"}):print()
```
