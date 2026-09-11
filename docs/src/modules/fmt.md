# `fmt` — formatting

Text formatting for terminal output: ANSI colours and styles, status symbols,
character-aware string helpers and aligned text tables. Everything is pure
computation; only colour *auto-detection* and `fmt.is_tty` read the host.

| Function | Availability | Effect |
| --- | --- | --- |
| `fmt.colors([on])` | everywhere | Set/read the colour switch; returns the resolved state. |
| `fmt.is_tty([stream])` | not in `ied` | Whether `"stdout"` (default), `"stderr"` or `"stdin"` is a terminal. |
| `fmt.red(s[, on])` … `s:red([on])` | everywhere | Styles: 16 foregrounds, 16 backgrounds, 6 attributes. |
| `fmt.if_color(s[, alt])` / `fmt.if_nocolor(s[, alt])` | everywhere | Pick `s` or `alt` on the colour switch. |
| `fmt.ok()` `fmt.fail()` `fmt.warn()` `fmt.info()` | everywhere | A status symbol with an ASCII fallback. |
| `fmt.strip(s)` / `s:without_ansi()` | everywhere | Remove ANSI escape sequences. |
| `fmt.width(s)` | everywhere | Display width, escapes excluded. |
| `fmt.trimmed(s[, chars])` `trimmed_start` `trimmed_end` | everywhere | Trim whitespace (or a character set). |
| `fmt.truncated(s, len[, ellipsis])` | everywhere | Keep at most `len` visible characters. |
| `fmt.cropped(s, start[, len])` | everywhere | Substring by 1-based character index. |
| `fmt.replaced_at(s, index, sub)` | everywhere | Replace one character. |
| `fmt.padded_start(s, len[, pad])` / `fmt.padded_end(s, len[, pad])` | everywhere | Pad to `len` characters. |
| `fmt.removed(s, sub[, n])` / `fmt.replaced(s, from, to[, n])` | everywhere | Literal removal / replacement. |
| `fmt.starts_with(s, p)` `ends_with` `contains` | everywhere | Literal tests. |
| `fmt.without_prefix(s, p)` / `without_suffix(s, p)` | everywhere | Drop a prefix/suffix if present. |
| `fmt.lines(s)` | everywhere | Split into lines. |
| `fmt.table.new(columns)` | everywhere | Start a table builder. |

Every function above that takes a string first is also a `string` method
(`s:red()`, `s:padded_end(8)`, …): the methods are sugar, `fmt` owns the
implementation.

## Example

```luau
local name = ("  Alice  "):trimmed()
print(fmt.ok() .. " " .. name:green())
-- terminal: a green ✔, then "Alice" in green
-- pipe:     "[ok] Alice", no escape codes at all

local t = fmt.table.new({"name", "age"})
t:rows({{name = "Ada", age = 36}, {name = "Grace", age = 85}})
t:align("age", fmt.table.RIGHT)
print(t:to_string())
```

## Conventions

- **Visible characters.** Positions and lengths in the text helpers count
  Unicode characters (`chars()`), not bytes, and ignore ANSI escapes.
  Escapes are never cut in half: `cropped`/`truncated`/`replaced_at` keep
  every escape sequence of a styled string and drop only the visible
  characters outside the selection, so a style opened before the cut is
  still closed after it. `padded_*` adds its padding outside the styled
  text. Invalid UTF-8 raises.
- **One process-wide colour switch**, read at call time via `fmt.colors()`.
  When off, every style is the identity function — a script writes
  `s:red()` unconditionally and the switch decides.
- **Auto differs per binary.** `fmt.colors(nil)` (the default) detects
  (CLICOLORS convention) but resolves to **off** under `ied`, whose runs
  must not depend on ambient state
  ([reproducibility](../binaries/ied.md#reproducibility)). A script there opts in
  with `fmt.colors(true)`.
- **Styles nest across categories.** Each style closes with the *specific*
  reset (`39` foreground, `49` background, `22`/`23`/`24`/`27`/`29` per
  attribute), so `("x"):red():bold()` keeps both. Within one category the
  inner reset ends the outer style: `("a" .. ("b"):blue() .. "c"):red()`
  leaves `c` uncoloured.
- **Table output is pure.** A table has its natural width unless you pass a
  width; it never consults the terminal.

## `fmt.colors`

Set or read the colour switch.

**Signature:** `fmt.colors([on]) -> boolean`

**Parameters:** `on` — `true` forces styling on, `false` off, `nil`
resets to *auto* (stdout), `"stdout"` is the same, `"stderr"` is auto
detection against **stderr** (keeps `NO_COLOR`/`CLICOLOR*`, unlike
`fmt.colors(fmt.is_tty("stderr"))`). No argument only reads — distinct from
passing `nil`.

**Returns:** the resolved boolean — "will my styles show up".

**Errors:** raises on a non-boolean, non-nil, non-stream argument, an
unknown stream name, or more than one argument.

**Auto detection**, highest precedence first: `CLICOLOR_FORCE`
set and not `0` → on; `NO_COLOR` set to a **non-empty** value → off
(<https://no-color.org>; an empty value does not count); `CLICOLOR=0` → off;
otherwise on iff the chosen stream is a terminal.

```luau
fmt.colors(true)            -- force on regardless of the pipe
fmt.colors("stderr")        -- auto, following stderr (log output)
if fmt.colors() then ... end
fmt.colors(nil)             -- back to auto
```

## `fmt.is_tty`

Whether a standard stream is a terminal. Not available in `ied` (a terminal
is ambient state, see [reproducibility](../binaries/ied.md#reproducibility)),
so `if fmt.is_tty then` is a usable feature test.

**Signature:** `fmt.is_tty([stream]) -> boolean`

**Parameters:** `stream` — `"stdout"` (default), `"stderr"` or `"stdin"`.

**Errors:** raises on any other stream name.

```luau
if fmt.is_tty() then print("interactive") end
```

## Styles

Wrap text in an SGR code.

**Signature:** `s:red([on]) -> string`, or `fmt.red(s[, on])`.

**Names:** foregrounds `black` `red` `green` `yellow` `blue` `magenta` `cyan`
`white` and a `bright_` counterpart of each; backgrounds `bg_` + the same
sixteen (`bg_red`, `bg_bright_white`); attributes `bold` `dim` `italic`
`underline` `reverse` `strike`.

**Parameters:** `on` (boolean, optional) — `false` opts this one call out of
styling. It is not a symmetric override: `true` does **not** force codes on
when the switch is off. An explicit `nil` counts as `false` (so
`s:green(is_live)` is plain when `is_live` is unset); only omission means
"on".

**Returns:** with styling on, the text wrapped as `"\27[<code>m" .. s ..
"\27[<reset>m"`; otherwise the text unchanged. `bold`/`dim` share reset `22`
(SGR has one "normal intensity").

**Errors:** raises if `on` is not a boolean or nil.

```luau
("this is red"):red() .. " this is normal"
("x"):red():bold()                       -- both survive
("status"):green(is_live):red(not is_live)
```

## `fmt.if_color` / `fmt.if_nocolor`

Pick between two texts on the colour switch.

**Signature:** `s:if_color([alt]) -> string`, `s:if_nocolor([alt]) -> string`

**Parameters:** `alt` — the other branch; defaults to `""`.

**Returns:** `if_color` returns `s` when styling is on, else `alt`;
`if_nocolor` is the mirror. Both are **eager selectors**, not guards: Luau
evaluates both sides first, and the receiver is not inspected for escapes.
For an expensive branch use a plain `if fmt.colors() then`.

```luau
("✔"):green():if_color("[ok]")   -- styled ✔ when on, "[ok]" when off
("-"):if_nocolor("─")            -- "-" when off, "─" when on
```

## Status symbols

**Signature:** `fmt.ok()`, `fmt.fail()`, `fmt.warn()`, `fmt.info()` -> string

**Returns:** with styling on, a coloured glyph (`✔` green, `✘` red, `!`
yellow, `ℹ` cyan); with it off, the ASCII fallback `[ok]`, `[fail]`,
`[warn]`, `[info]`.

```luau
print(fmt.ok() .. " deployed")
```

## `fmt.strip`

Remove ANSI escape sequences (method form: `s:without_ansi()`).

**Signature:** `fmt.strip(s) -> string`

**Returns:** `s` without CSI sequences (`\27[` … final byte, also the C1
`U+009B`), OSC sequences (`\27]` … BEL or `\27\`, e.g. OSC 8 hyperlinks —
the link text is kept), nF escapes and other two-byte escapes. An
unterminated sequence runs to the end of the string and is dropped whole.

```luau
fmt.strip(("ok"):green())   -- "ok"
```

## `fmt.width`

Display width of a string, escapes excluded.

**Signature:** `fmt.width(s) -> integer`

**Returns:** the sum of Unicode character widths (wide CJK and emoji
count 2; combining marks, tabs, newlines and other controls count 0). It is
the width of the string as if on one line: for multi-line text take the
maximum over `fmt.lines(s)`. The same measure the table renderer uses.

```luau
fmt.width(("ok"):green())   -- 2 (not the byte length)
fmt.width("日本")            -- 4
```

## `fmt.trimmed`

**Signature:** `fmt.trimmed(s[, chars]) -> string`, likewise `trimmed_start`
and `trimmed_end` (leading / trailing only).

**Parameters:** `chars` — a string whose characters are all trimmed
instead. Without it, Unicode whitespace (Rust's `char::is_whitespace`,
which is wider than Luau's `%s`).

```luau
("  hi  "):trimmed()        -- "hi"
("--x--"):trimmed("-")      -- "x"
```

## `fmt.truncated`

Keep at most `len` visible characters.

**Signature:** `fmt.truncated(s, len[, ellipsis]) -> string`

**Parameters:** `len` — maximum visible length (`<= 0` gives `""`).
`ellipsis` — text appended in place of the removed tail; it counts toward
`len`, and is itself cut if longer than `len`. A string that already fits is
returned unchanged.

```luau
("hello"):truncated(3)          -- "hel"
("hello"):truncated(4, "…")     -- "hel…"
```

## `fmt.cropped`

Substring by 1-based character index.

**Signature:** `fmt.cropped(s, start[, len]) -> string`

**Parameters:** `start` — 1-based index; negative counts from the end (`-1`
is the last character). `len` — a **length** in characters (not an end index,
unlike `string.sub`); omitted takes the rest. Both clamp into range; nothing
raises.

```luau
("hello"):cropped(-3)       -- "llo"
("hello"):cropped(2, 3)     -- "ell"
```

## `fmt.replaced_at`

Replace the character at one index.

**Signature:** `fmt.replaced_at(s, index, sub) -> string`

**Parameters:** `index` — 1-based, negative from the end. `sub` — any text.

**Errors:** raises if `index` is `0` or out of range.

```luau
("hello"):replaced_at(1, "J")    -- "Jello"
```

## `fmt.padded_start` / `fmt.padded_end`

Pad to a visible length on the left / right.

**Signature:** `fmt.padded_end(s, len[, pad]) -> string`

**Parameters:** `len` — target length. `pad` — text repeated and cut to fit;
defaults to `" "`.

**Returns:** `s` unchanged if already `len` or longer.

**Errors:** raises if `pad` is empty.

```luau
("42"):padded_start(5, "0")     -- "00042"
("ab"):padded_end(4)            -- "ab  "
```

## `fmt.removed` / `fmt.replaced`

**Signature:** `fmt.removed(s, sub[, n])`, `fmt.replaced(s, from, to[, n])`

**Parameters:** `sub`/`from` are **literal** text, not patterns (use `re`
for patterns). `n` — replace only the first `n` matches (default all). An
empty `sub`/`from` returns `s` unchanged.

**Errors:** raises if `n` is negative.

```luau
("a-b-c"):replaced("-", "_", 1)    -- "a_b-c"
```

## `fmt.starts_with` / `ends_with` / `contains`

Literal tests: `fmt.starts_with(s, p) -> boolean`, `ends_with`, `contains`.

## `fmt.without_prefix` / `without_suffix`

`fmt.without_prefix(s, p) -> string` removes `p` from the start if present,
else returns `s`; `without_suffix` likewise at the end.

## `fmt.lines`

**Signature:** `fmt.lines(s) -> {string}`

**Returns:** the lines without their `\n` / `\r\n` terminators; a
trailing newline does not produce a final empty line.

```luau
("a\nb\n"):lines()    -- {"a", "b"}
```

## `fmt.table.new`

Start a table builder. (Named `fmt.table` because `table` is Luau's own
library.)

**Signature:** `fmt.table.new(columns: {string}) -> Builder`, or
`fmt.table.new(...: string)`

**Parameters:** `columns` — the column **keys**, in display order: the names
a row is looked up by and the default header text.

**Errors:** raises if `columns` is empty.

**Methods:** `headers` · `header` · `row` · `rows` · `align` · `align_header` ·
`align_cell` · `column_width` · `padding` · `preset` · `modifier` · `to_string`

**Builder rules:**

- A row is **keyed** (`{name = "Ada", age = 36}`) or **positional**
  (`{"Ada", 36}`); a positional value is used only where the key is absent.
  A missing field is an empty cell.
- A cell or header is a string, number, boolean or `nil` (empty); anything
  else raises, as does an unknown key, a positional value past the last
  column, or an empty row.
- Every builder method mutates the table in place and returns the same
  handle, so calls chain; a bad value (an unknown column, a malformed
  alignment) raises at that call. `:to_string()` is the terminal
  (`tostring(t)` is `t:to_string()`).
- Constants live on `fmt.table`: alignments `LEFT` `CENTER` `RIGHT`; presets
  `ASCII_FULL` `ASCII_FULL_CONDENSED` `ASCII_NO_BORDERS` `ASCII_BORDERS_ONLY`
  `ASCII_BORDERS_ONLY_CONDENSED` `ASCII_HORIZONTAL_ONLY` `ASCII_MARKDOWN`
  `UTF8_FULL` (default) `UTF8_FULL_CONDENSED` `UTF8_NO_BORDERS`
  `UTF8_BORDERS_ONLY` `UTF8_HORIZONTAL_ONLY` `NOTHING` (no border
  characters; the one-character column padding remains); modifiers
  `ROUND_CORNERS` `SOLID_INNER_BORDERS`.
- Width counting is `fmt.width`'s, ANSI-aware.

```luau
local t = fmt.table.new({"host", "status"})
```

## `t:headers` / `t:header`

**Signature:** `t:headers(names: {string}) -> t` (or varargs),
`t:headers(nil) -> t`, `t:header(col, name) -> t`

**Parameters:** `names` — one header per column. `nil` omits the header row
entirely; calling `headers()` with **no** argument raises. `col` — a 1-based
index or key name.

**Errors:** a wrong number of names, an unknown `col`, or `header` after
`headers(nil)`.

```luau
t:headers({"Host", "Status"})
t:header("status", "State")
```

## `t:row` / `t:rows`

**Signature:** `t:row(data: table) -> t` (or positional varargs),
`t:rows(data: {table}) -> t`

**Errors:** a non-list `data` (including a map), a non-table element, or a
row that fails the rules above.

```luau
t:row({host = "web-1", status = "ok"})
t:rows({{"web-2", "down"}, {"web-3", "ok"}})
```

## `t:align` / `t:align_header` / `t:align_cell`

**Signature:** `t:align(col, alignment)` (whole column),
`t:align_header(col, alignment)`, `t:align_cell(row, col, alignment)` (one
cell; `row` is the 1-based index in insertion order) — all return `t`.

**Errors:** an out-of-range or unknown `col`/`row`, or an alignment that is
not one of the constants.

```luau
t:align("age", fmt.table.RIGHT)
t:align_header("age", fmt.table.RIGHT)
```

## `t:column_width` / `t:padding`

`t:column_width(col, n)` fixes a column's content width (`n >= 1`);
`t:padding(col, left, right)` sets its padding in characters. Both return
`t`.

## `t:preset` / `t:modifier`

`t:preset(name)` switches the border style; `t:modifier(name)` applies
`ROUND_CORNERS` or `SOLID_INNER_BORDERS`. Both return `t` and raise on an
unknown name.

```luau
fmt.table.new({"a", "b"}):preset(fmt.table.ASCII_MARKDOWN)
```

## `t:to_string`

**Signature:** `t:to_string([width]) -> string`

**Parameters:** `width` — target total width, an integer `1..65535`; columns
wrap to fit. Omitted: the natural width, independent of any terminal.

**Errors:** raises on a zero, negative, fractional or too-large width.

There is no `:print`: use `print(t:to_string())`.

```luau
print(fmt.table.new({"a"}):row({"x"}):to_string())
```
