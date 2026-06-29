# `string` — strings

Strings are Luau built-ins. ito adds pure helper functions and ANSI
styling methods onto the same built-in `string` table — it does not
replace anything Luau already provides.

| Function/method | Effect |
| --- | --- |
| `string.trimmed(s)` | Strip leading and trailing whitespace. |
| `string.trimmed_start(s)` | Strip leading whitespace. |
| `string.trimmed_end(s)` | Strip trailing whitespace. |
| `string.truncated(s, len)` | Keep the first `len` characters. |
| `string.cropped(s, start[, len])` | Substring by 1-based character index (negative counts from the end). |
| `string.set(s, index, sub)` | Replace the character at `index` with `sub`. |
| `string.padded(s, len, pad)` | Right-pad with `pad` up to `len` characters. |
| `string.removed(s, sub)` | Remove every occurrence of `sub`. |
| `string.replaced(s, from, to)` | Replace every occurrence of `from` with `to`. |
| `black` `red` `green` `yellow` `blue` `magenta` `cyan` `white` | Set foreground colour. |
| `bright_black` … `bright_white` | Bright foreground variant. |
| `bg_black` … `bg_white`, `bg_bright_black` … `bg_bright_white` | Set background colour. |
| `bold` `dim` `italic` `underline` `reverse` `strike` | Set an attribute. |
| all of the above take `s:red([on])` | `on = false` opts this one call out of styling; it cannot force styling on. |
| `s:if_color([alt])` | The receiver when styling is on, else `alt` (or `""`). |
| `s:if_nocolor([alt])` | The receiver when styling is off, else `alt` (or `""`). |
| `s:stripped()` | Remove every ANSI escape sequence. |
| `s:width()` | Display width in characters, escapes excluded. |

`upper`/`lower` are not added — Luau's built-in `string.upper`/
`string.lower` already cover them.

```lua
local name = ("  Alice  "):trimmed()
print(("[ok]"):green() .. " " .. name)
-- on a terminal:  a green "[ok]" followed by " Alice"
-- piped to a file: "[ok] Alice", no escape codes at all
```

## General rules

- **Everything here is pure.** Every function/method takes a string
  (plus arguments) and returns a new string; Luau strings are
  immutable, so there is no in-place variant to distinguish.
- **Character-aware, not byte-aware.** `cropped`/`set`/`padded`/
  `truncated` index Unicode `chars()`, unlike `string.sub` which
  indexes bytes and can split a multi-byte codepoint in two. Safe on
  non-ASCII text.
- **Method syntax works everywhere.** All of these live on the
  `string` table that backs every string value's metatable, so
  `("  hi  "):trimmed()` and `string.trimmed("  hi  ")` are the same
  call.
- **Styling nests via specific resets.** Each styling method wraps its
  receiver in a set/reset pair and closes with the *specific* reset for
  what it set — `39` for a foreground, `49` for a background, `22`/
  `23`/`24`/`27`/`29` per attribute — never the blanket `0`. That is
  what lets `("x"):red():bold()` keep both: the inner reset does not
  clear the outer style.
- **One process-wide switch decides whether styling emits anything**,
  read at call time via `ito.colors()` (documented in the `ito`
  module). When it is off, every styling method is the identity
  function — a script writes `s:red()` unconditionally and the switch
  decides, rather than an `if` at each call site.
- **Each styling method also takes an optional trailing boolean** —
  `s:red(on)` — to opt just that one call out of styling
  (`s:red(false)` is always plain, whatever the switch says). It is
  **not** a symmetric override: `s:red(true)` does not force codes on
  when the switch itself is off. The result is
  `ito.colors() and on ~= false`, so `on` can only turn a call *off*,
  never turn it on against the switch.
- **Auto differs per binary.** `ito.colors(nil)` (the default) detects
  under `ito`/`irun` (CLICOLORS convention, ambient state) but resolves
  to **off** under `iq`/`ied`, which deliberately cannot see ambient
  state — `iq`'s output is its return value, and an `ied --diff` must
  not depend on what the run was piped into. A script under `iq`/`ied`
  opts in explicitly with `ito.colors(true)`.
- **`if_color`/`if_nocolor` are eager selectors, not guards.** Luau has
  already evaluated the receiver and the argument by the time either
  method runs, so they pick between two existing strings — they do not
  defer or skip computing a branch. Neither inspects the receiver for
  escape sequences; the switch alone decides which branch wins.
- **`width()` is escape-aware, not a full terminal width model.** It
  counts `chars()` after stripping escapes, so combining marks and
  double-width CJK characters each count once — no `unicode-width`
  behavior.

## `string.trimmed`

Strip leading and trailing whitespace.

**Signature:** `s:trimmed() -> string`

**Parameters:** none besides the receiver.

**Returns:** a new string with whitespace trimmed from both ends.

```lua
string.trimmed("  hello  ")   -- "hello"
```

## `string.trimmed_start`

Strip leading whitespace only.

**Signature:** `s:trimmed_start() -> string`

**Returns:** a new string with leading whitespace removed.

```lua
string.trimmed_start("  hello  ")   -- "hello  "
```

## `string.trimmed_end`

Strip trailing whitespace only.

**Signature:** `s:trimmed_end() -> string`

**Returns:** a new string with trailing whitespace removed.

```lua
string.trimmed_end("  hello  ")   -- "  hello"
```

## `string.truncated`

Keep the first `len` characters.

**Signature:** `s:truncated(len) -> string`

**Parameters:**
- `len` (integer) — number of characters to keep. Negative or zero
  yields `""`; a `len` longer than the string returns the whole string.

**Returns:** the first `len` characters (character count, not bytes).

```lua
string.truncated("hello", 3)    -- "hel"
string.truncated("hi", 10)      -- "hi"
```

## `string.cropped`

Substring by 1-based character index, matching `string.sub`'s
convention but indexing whole characters instead of bytes.

**Signature:** `s:cropped(start[, len]) -> string`

**Parameters:**
- `start` (integer) — 1-based character index. Negative counts from
  the end (`-1` is the last character). Clamped into range rather than
  raising.
- `len` (integer, optional) — number of characters to take from
  `start`. Omitted (`nil`) takes to the end of the string.

**Returns:** the selected substring, character-safe on non-ASCII text
(unlike `string.sub`, which can split a multi-byte codepoint).

```lua
string.cropped("hello", -3)     -- "llo"
string.cropped("hello", 1, 3)   -- "hel"
```

## `string.set`

Replace the character at a single index with a (possibly
multi-character) substitute.

**Signature:** `s:set(index, sub) -> string`

**Parameters:**
- `index` (integer) — 1-based character index of the character to
  replace. Negative counts from the end.
- `sub` (string) — replacement text; need not be a single character.

**Returns:** a new string with the character at `index` replaced by
`sub`. If `index` is out of range, returns the original string
unchanged (no error).

```lua
string.set("hello", 1, "H")     -- "Hello"
string.set("hello", -1, "!")    -- "hell!"
```

## `string.padded`

Right-pad with a repeating pad string up to a target character count.

**Signature:** `s:padded(len, pad) -> string`

**Parameters:**
- `len` (integer) — target length in characters. `len <= 0` returns
  the receiver unchanged.
- `pad` (string) — pattern to repeat while padding. Empty `pad`
  returns the receiver unchanged. If the receiver is already `len`
  characters or longer, it is returned unchanged (never truncates).

**Returns:** the receiver padded on the right with repetitions of
`pad`, cut off mid-pattern if needed to land exactly on `len`.

```lua
string.padded("42", 5, "0")     -- "42000"
string.padded("42", 5, "ab")    -- "42aba"
string.padded("hello", 3, "0")  -- "hello" (already long enough)
```

## `string.removed`

Remove every occurrence of a substring.

**Signature:** `s:removed(sub) -> string`

**Parameters:**
- `sub` (string) — substring to remove. Empty `sub` returns the
  receiver unchanged (no infinite matching against every position).

**Returns:** the receiver with every occurrence of `sub` deleted.

```lua
string.removed("hello world", "o")   -- "hell wrld"
```

## `string.replaced`

Replace every occurrence of one substring with another.

**Signature:** `s:replaced(from, to) -> string`

**Parameters:**
- `from` (string) — substring to find. Empty `from` returns the
  receiver unchanged.
- `to` (string) — replacement text, inserted at every match.

**Returns:** the receiver with every occurrence of `from` replaced by
`to`.

```lua
string.replaced("a-b-c", "-", "_")   -- "a_b_c"
```

## Foreground colours (`black` … `white`, `bright_*`)

Wrap the receiver in an SGR foreground colour code.

**Signature:** `s:red([on]) -> string` (and `black`, `green`, `yellow`,
`blue`, `magenta`, `cyan`, `white`, plus a `bright_` counterpart for
each — 16 methods total).

**Parameters:**
- `on` (boolean, optional) — pass `false` to opt this one call out of
  styling regardless of `ito.colors()`. Passing `true`, `nil`, or
  omitting it defers to the switch; `true` does **not** force styling
  on when the switch is off.

**Returns:** when styling is on (`ito.colors()` true and `on ~= false`),
the receiver wrapped as `"\27[<code>m" .. s .. "\27[39m"`; otherwise the
receiver unchanged. Never raises.

```lua
("this is red"):red() .. " this is normal"
("warning"):bright_yellow()
-- pick the style with a condition instead of an if/else around the call
("status"):green(is_live):red(not is_live)
```

## Background colours (`bg_black` … `bg_bright_white`)

Wrap the receiver in an SGR background colour code.

**Signature:** `s:bg_red([on]) -> string` (16 methods total: `bg_` +
each of the 8 names, plus `bg_bright_` + each of the 8 names).

**Parameters:** `on` (boolean, optional) — same opt-out-only semantics
as the foreground colours above.

**Returns:** when styling is on, the receiver wrapped with reset `49`
(background default); when off, unchanged. Never raises.

```lua
("label"):bg_blue()
("alert"):bg_bright_red()
```

## Attributes (`bold`, `dim`, `italic`, `underline`, `reverse`, `strike`)

Wrap the receiver in an SGR attribute code.

**Signature:** `s:bold([on]) -> string` (and `dim`, `italic`,
`underline`, `reverse`, `strike`).

**Parameters:** `on` (boolean, optional) — same opt-out-only semantics
as the foreground colours above.

**Returns:** when styling is on, the receiver wrapped with its
*specific* reset code (`bold`/`dim` both reset via `22` — SGR has one
"normal intensity" and no way to clear only one of a pair set
together; `italic`→`23`, `underline`→`24`, `reverse`→`27`,
`strike`→`29`); when off, unchanged. Never raises.

```lua
("x"):red():bold()       -- both survive: each closes with its own reset
("label"):bold():bg_blue()
```

## `string.if_color`

Pick the receiver when styling is on.

**Signature:** `s:if_color([alt]) -> string`

**Parameters:**
- `alt` (string, optional) — text to use when styling is off. Defaults
  to `""`.

**Returns:** the receiver if styling is on, else `alt` (or `""`).
Both the receiver and `alt` are evaluated by Luau before the call —
this method *selects*, it does not defer evaluation of either side.
Does not inspect the receiver for escape sequences; the switch alone
decides.

```lua
("✔"):green():if_color("[ok]")   -- styled "✔" when on, "[ok]" when off
("x"):if_color()                 -- "" when styling is off
```

## `string.if_nocolor`

Pick the receiver when styling is off — the mirror of `if_color`.

**Signature:** `s:if_nocolor([alt]) -> string`

**Parameters:**
- `alt` (string, optional) — text to use when styling is on. Defaults
  to `""`.

**Returns:** the receiver if styling is off, else `alt` (or `""`).
Same eager-selection and no-inspection behavior as `if_color`.

```lua
("─"):if_nocolor("-")            -- box-drawing char when on, "-" when off
("(no colour)"):if_nocolor()     -- prints only when colours are off
```

Combined idiom, correct on both a terminal and a plain pipe:

```lua
("✔"):green():if_color("[ok]")
```

If a branch is itself expensive to compute, don't rely on these to
skip the work — both sides are always evaluated. Ask the switch
directly instead:

```lua
local bar = if ito.colors() then render_blocks(pct) else render_ascii(pct) end
```

## `string.stripped`

Remove every ANSI escape sequence.

**Signature:** `s:stripped() -> string`

**Parameters:** none besides the receiver.

**Returns:** the receiver with CSI sequences (`\27[` … final byte in
`@`–`~`, including multi-parameter forms like 256-colour/truecolour)
and OSC sequences (`\27]` … BEL or `\27\`, e.g. OSC 8 hyperlinks — text
between the markers is kept) removed. An unterminated CSI runs to the
end of the string and is dropped whole, matching what a terminal would
do with it. Other two-byte escapes are dropped entirely. Never raises.

```lua
("ok"):green():stripped()   -- "ok"
```

## `string.width`

Display width in characters, with escape sequences excluded.

**Signature:** `s:width() -> number`

**Parameters:** none besides the receiver.

**Returns:** the character count (`chars()`, not bytes) of the
receiver after stripping ANSI escapes. A multi-byte codepoint counts
once. This is not a full terminal-width model — no double-width CJK
or combining-mark awareness beyond one-codepoint-one-count.

```lua
local cell = ("ok"):green()
#cell           -- 12 (bytes, escapes included)
cell:width()    -- 2  (what the terminal actually shows)
```

Use `width()`, not `#s`, when sizing a styled table column — `#s`
counts the escape bytes and makes a styled cell look far wider than it
prints.
