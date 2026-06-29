# `ui.prompt` — interactive prompts

**Host-only (`ito`/`irun`).** `ui` names ambient terminal state — a prompt
blocks on stdin — that a query's return value or an edit preview must not
depend on, so it is absent rather than stubbed under `iq`/`ied`: `if ui
then` is a usable feature test.

Every prompt kind offers two layers: a **shorthand** function for the
common case, and a **builder** (`<kind>_builder()`) exposing every knob
that kind supports. Both block the calling task on the terminal, so a
script doing several prompts in sequence just calls them in sequence —
there is no form that runs concurrently with itself, since a terminal has
one cursor.

| Shorthand | Builder | Returns |
| --- | --- | --- |
| `ui.prompt.confirm(msg[, default])` | `ui.prompt.confirm_builder()` | `bool` |
| `ui.prompt.input(msg[, default])` | `ui.prompt.input_builder()` | `string` |
| `ui.prompt.password(msg[, confirm])` | `ui.prompt.password_builder()` | `string` |
| `ui.prompt.select(msg, items[, default])` | `ui.prompt.select_builder()` | `1`-based index |
| `ui.prompt.multi_select(msg, items[, defaults])` | `ui.prompt.multi_select_builder()` | array of `1`-based indices |

`select`/`multi_select` index **1-based**, matching every other ito surface
(`path.capture`, `cli` positionals, Luau itself).

```lua
if ui.prompt.confirm("Deploy to prod?") then
    -- ...
end
```

### Errors

**Ctrl-C** raises a Luau error rather than returning a default — wrap the
call in `pcall` for "abort gracefully". **Escape** is different: plain
`:interact()` also raises for it, but `select`/`multi_select` builders
additionally offer `:interact_opt()`, which returns `nil` instead of
raising — the one gesture treated as a first-class "no answer" on those two
kinds.

`select`/`multi_select` (both shorthand and builder) raise if `items` is
empty, and if a `default`/pre-checked index is out of range for `items`,
before ever touching the terminal.

A prompt's result is not validated for you — check it and re-prompt in a
loop for anything beyond "some text was entered":

```lua
local n
repeat
    n = tonumber(ui.prompt.input("Port"))
until n and n > 0 and n < 65536
```

## `ui.prompt.confirm` / `confirm_builder`

A yes/no prompt.

**Shorthand:** `ui.prompt.confirm(msg[, default]) -> bool`

**Builder methods:** `:prompt(msg)` · `:default(bool)` ·
`:show_default(bool)` (default `true`) · `:wait_for_newline(bool)` (default
`false`; when `true`, `y`/`n` alone does not answer — Enter is required) ·
`:report(bool)` (default `true`; whether the chosen answer is echoed back
after the prompt closes) · `:theme(name)` · `:interact()` · `:interact_opt()`

```lua
if ui.prompt.confirm("Deploy to prod?", false) then
    -- ...
end

local ok = ui.prompt.confirm_builder()
    :prompt("Really delete everything?")
    :default(false)
    :wait_for_newline(true)
    :interact()
```

## `ui.prompt.input` / `input_builder`

Free-text input.

**Shorthand:** `ui.prompt.input(msg[, default]) -> string`

**Builder methods:** `:prompt(msg)` · `:default(string)` ·
`:initial_text(string)` (pre-filled and editable, unlike `:default` which
is only offered on Enter) · `:allow_empty(bool)` (default `false`) ·
`:show_default(bool)` (default `true`) ·
`:post_completion_text(string)` (replaces the prompt text once answered) ·
`:report(bool)` (default `true`) · `:theme(name)` · `:interact()`

```lua
local name = ui.prompt.input("Service name", "web")

local token = ui.prompt.input_builder()
    :prompt("API token")
    :allow_empty(false)
    :interact()
```

## `ui.prompt.password` / `password_builder`

Input with the typed characters hidden.

**Shorthand:** `ui.prompt.password(msg[, confirm]) -> string` — `confirm`
(default `false`), when `true`, asks a second time and re-prompts on
mismatch rather than raising.

**Builder methods:** `:prompt(msg)` · `:allow_empty_password(bool)`
(default `false`) · `:with_confirmation(confirm_prompt, mismatch_message)`
· `:report(bool)` (default `true`) · `:theme(name)` · `:interact()`

```lua
local pw = ui.prompt.password("New password", true)

local pw2 = ui.prompt.password_builder()
    :prompt("New password")
    :with_confirmation("Confirm password", "passwords didn't match")
    :interact()
```

## `ui.prompt.select` / `select_builder`

Choose one item from a list, returning its **1-based** index.

**Shorthand:** `ui.prompt.select(msg, items, [default]) -> integer`

**Builder methods:** `:prompt(msg)` · `:items(list)` · `:item(v)` (append
one) · `:default(i)` (1-based) · `:max_length(n)` (visible rows before
scrolling) · `:clear(bool)` (default `true`; erase the item list from the
screen after answering — a `select`/`multi_select`-only knob, since these
are the two prompts that draw more than one line to begin with) ·
`:report(bool)` (default `true`) · `:theme(name)` · `:interact() ->
integer` · `:interact_opt() -> integer?`

```lua
local i = ui.prompt.select("Environment", { "dev", "staging", "prod" })
print(({ "dev", "staging", "prod" })[i])

local i = ui.prompt.select_builder()
    :prompt("Environment")
    :items({ "dev", "staging", "prod" })
    :default(1)
    :interact_opt()   -- nil if the user pressed Escape
```

## `ui.prompt.multi_select` / `multi_select_builder`

Choose any number of items, returning an array of **1-based** indices.

**Shorthand:** `ui.prompt.multi_select(msg, items[, defaults]) -> integer[]`
— `defaults` is a list of 1-based indices to pre-check.

**Builder methods:** `:prompt(msg)` · `:items(list)` · `:item(v[,
checked])` (append one, optionally pre-checked) · `:checked(i)` (pre-check
the `i`'th, 1-based) · `:defaults(list)` (the whole pre-checked set at
once, replacing any set so far) · `:max_length(n)` · `:clear(bool)`
(default `true`) · `:report(bool)` (default `true`) · `:theme(name)` ·
`:interact() -> integer[]` · `:interact_opt() -> integer[]?`

```lua
local picked = ui.prompt.multi_select("Regions", { "eu", "us", "ap" }, { 1 })

local picked = ui.prompt.multi_select_builder()
    :prompt("Regions")
    :items({ "eu", "us", "ap" })
    :checked(1)
    :interact()
```

## Themes

Every builder (and both shorthands) draw through `:theme(name)`, one of:

- `"auto"` (**default**) — tracks `ito.colors()`, the same switch
  `string:red()` reads: colourful when that resolves to on, plain
  otherwise. So a prompt's colour follows `NO_COLOR`/a piped stdout/an
  explicit `ito.colors(false)` exactly like every other styled output.
- `"colorful"` — a colourful theme, unconditionally.
- `"simple"` — no colour, unconditionally.
