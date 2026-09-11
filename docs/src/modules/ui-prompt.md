# `ui.prompt` — interactive prompts

| Shorthand | Builder | Returns |
| --- | --- | --- |
| `ui.prompt.confirm(msg[, default])` | `ui.prompt.confirm_builder()` | `boolean` |
| `ui.prompt.input(msg[, default])` | `ui.prompt.input_builder()` | `string` |
| `ui.prompt.password(msg[, confirm])` | `ui.prompt.password_builder()` | `string` |
| `ui.prompt.select(msg, items[, default])` | `ui.prompt.select_builder()` | `index, item` |
| `ui.prompt.multi_select(msg, items[, defaults])` | `ui.prompt.multi_select_builder()` | array of indices |

Not available in `ied`: a prompt reads the terminal, ambient state an `ied`
run must not depend on ([reproducibility](../binaries/ied.md#reproducibility)).
`ui` is absent rather than stubbed, so `if ui then` is a usable feature test.

## Example

```lua
if ui.prompt.confirm("Deploy to prod?") then
    local name = ui.prompt.input("Service name", "web")
end
```

## Conventions

- **Two layers.** Each kind has a shorthand for the common case and a builder
  (`<kind>_builder()`) for every knob. Builder setters are unprefixed
  (`:prompt()`, `:default()`), mutate a shared spec in place and return the
  same handle, so they chain and a later alias sees the change. They are
  order-independent: everything, index bounds included, is validated when
  `:interact()` runs. The shorthands use the `auto` theme and the
  builders' defaults.
- **Indices are 1-based**, like the rest of ito.
- **Booleans are strict.** Flag arguments must be `true` or `false` (or `nil`
  where optional); `"false"` or `1` raise instead of being read as truthy.
- **A terminal is required.** Prompts draw on stderr. If it is not a terminal
  the prompt raises, naming the problem — unless the builder was given a
  `:default()` and `:or_default(true)` (`confirm`, `input`, `select`), in which
  case it answers with that default. `password` and `multi_select` have no
  such fallback.
- **One at a time.** Concurrent prompts (from several `task`s) are queued, and
  live [`ui.progress`](ui-progress.md) bars are paused while one is shown.
- **Ctrl-C raises a Luau error** (`pcall` it for a graceful abort); the
  terminal is restored (cursor shown) however the prompt ends.
- **Escape.** `:interact()` ignores Escape and keeps asking.
  `:interact_opt()` (`confirm`, `select`, `multi_select`) returns `nil` on
  Escape — or on `q`, which also quits there.
- **Abandoned prompts.** If the asking task is cancelled, the pending
  keyboard read cannot be interrupted and keeps consuming keys until one
  arrives.
- **Themes.** `:theme(name)`: `"auto"` (default; follows `fmt.colors()`),
  `"colorful"`, `"simple"`. A custom theme, live validators and
  history/completion are not available (they need callbacks into Luau from
  the terminal thread); validate the answer and ask again in a loop:
  ```lua
  local n
  repeat
      n = tonumber(ui.prompt.input("Port"))
  until n and n > 0 and n < 65536
  ```
- **`:report(true)`** (the default) echoes the answer after the prompt
  closes; it only shows when the prompt has a `:prompt()` text.

## `ui.prompt.confirm` / `confirm_builder`

A yes/no question.

**Signature:** `ui.prompt.confirm(msg: string[, default: boolean]) -> boolean`

**Methods:** `:prompt(msg)` · `:default(bool)` · `:show_default(bool)`
(default `true`) · `:wait_for_newline(bool)` (default `false`; when `true`, `y`/`n`
alone does not answer — Enter is required) · `:or_default(bool)` · `:report(bool)` ·
`:theme(name)` · `:interact() -> boolean` · `:interact_opt() -> boolean?`

**Returns:** the answer. Enter alone answers with the default, or keeps
asking if there is none.

**Errors:** no terminal (see above); Ctrl-C.

```lua
local ok = ui.prompt.confirm_builder()
    :prompt("Really delete everything?")
    :default(false)
    :wait_for_newline(true)
    :interact()
```

## `ui.prompt.input` / `input_builder`

Free-text input.

**Signature:** `ui.prompt.input(msg: string[, default: string]) -> string`

The shorthand rejects an empty answer unless there is a default; use the
builder's `:allow_empty(true)` to accept one.

**Methods:** `:prompt(msg)` · `:default(string)` · `:initial_text(string)`
(pre-filled and editable, unlike `:default`, which is only offered on Enter) ·
`:allow_empty(bool)` (default `false`) · `:show_default(bool)` (default `true`) ·
`:post_completion_text(string)` (replaces the prompt text once answered) ·
`:or_default(bool)` · `:report(bool)` · `:theme(name)` · `:interact() -> string`

```lua
local name = ui.prompt.input("Service name", "web")
local note = ui.prompt.input_builder():prompt("Note"):allow_empty(true):interact()
```

## `ui.prompt.password` / `password_builder`

Input with the typed characters hidden.

**Signature:** `ui.prompt.password(msg: string[, confirm: boolean]) -> string`

`confirm = true` asks a second time (prompt `Confirm`, mismatch message
`passwords didn't match`) and re-asks on mismatch. Use the builder for other
wording.

**Methods:** `:prompt(msg)` · `:allow_empty_password(bool)` (default
`false`) · `:with_confirmation(confirm_prompt, mismatch_message)` · `:report(bool)` ·
`:theme(name)` · `:interact() -> string`

```lua
local pw = ui.prompt.password_builder()
    :prompt("New password")
    :with_confirmation("Repeat", "they differ")
    :interact()
```

## `ui.prompt.select` / `select_builder`

Choose one item.

**Signature:** `ui.prompt.select(msg: string, items: {string}[, default: integer]) -> (integer, string)`

**Methods:** `:prompt(msg)` · `:items(list)` · `:item(v)` (append one) ·
`:default(i)` (1-based; the first item if never called) · `:max_length(n)`
(visible rows before scrolling) · `:clear(bool)` (default `true`; erase the
list after answering) · `:or_default(bool)` · `:report(bool)` · `:theme(name)` ·
`:interact() -> (integer, string)` · `:interact_opt() -> (integer?, string?)`

**Returns:** the 1-based index **and** the chosen item. `:interact_opt()`
returns `nil, nil` on Escape.

**Errors:** empty `items`; `default` outside `1..#items`; no terminal; Ctrl-C.

```lua
local i, env = ui.prompt.select("Environment", { "dev", "staging", "prod" })

local i = ui.prompt.select_builder()
    :prompt("Environment")
    :items({ "dev", "staging", "prod" })
    :default(1)
    :interact_opt()   -- nil if the user pressed Escape
```

## `ui.prompt.multi_select` / `multi_select_builder`

Choose any number of items.

**Signature:** `ui.prompt.multi_select(msg: string, items: {string}[, defaults: {integer}]) -> {integer}`

`defaults` lists the 1-based indices to pre-check.

**Methods:** `:prompt(msg)` · `:items(list)` · `:item(v[, checked])`
(append one, optionally pre-checked) · `:checked(i)` (pre-check the `i`'th) ·
`:defaults(list)` (replace the pre-checked set) · `:all()` (pre-check everything) ·
`:max_length(n)` · `:clear(bool)` (default `true`) · `:report(bool)` ·
`:theme(name)` · `:interact() -> {integer}` · `:interact_opt() -> {integer}?`

**Returns:** the chosen indices, ascending. `:interact_opt()` returns `nil` on
Escape.

**Errors:** empty `items`; a pre-checked index out of range; no terminal; Ctrl-C.

```lua
local picked = ui.prompt.multi_select("Regions", { "eu", "us", "ap" }, { 1 })

local all = ui.prompt.multi_select_builder()
    :prompt("Regions")
    :items({ "eu", "us", "ap" })
    :all()
    :interact()
```
