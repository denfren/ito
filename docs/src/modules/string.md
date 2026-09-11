# `string` — strings

Strings are Luau built-ins. ito adds helper methods onto the same built-in
`string` table — it does not replace anything Luau already provides.

Every added method is **sugar** for a function of the [`fmt`](fmt.md)
module, which owns the implementation and the documentation:
`("  hi  "):trimmed()` is `fmt.trimmed("  hi  ")`.

| Method | Effect |
| --- | --- |
| `trimmed` `trimmed_start` `trimmed_end` | trim whitespace — [`fmt.trimmed`](fmt.md#fmttrimmed) |
| `truncated` `cropped` `replaced_at` | cut/replace by visible character — [`fmt.truncated`](fmt.md#fmttruncated), [`fmt.cropped`](fmt.md#fmtcropped), [`fmt.replaced_at`](fmt.md#fmtreplaced_at) |
| `padded_start` `padded_end` | pad to a width — [`fmt.padded_start`](fmt.md#fmtpadded_start--fmtpadded_end) |
| `removed` `replaced` | literal removal/replacement — [`fmt.removed`](fmt.md#fmtremoved--fmtreplaced) |
| `starts_with` `ends_with` `contains` `without_prefix` `without_suffix` `lines` | literal tests and splitting — [`fmt.starts_with`](fmt.md#fmtstarts_with--ends_with--contains) |
| `width` `without_ansi` | display width / strip escapes — [`fmt.width`](fmt.md#fmtwidth), [`fmt.strip`](fmt.md#fmtstrip) |
| `red` `bold` `bg_blue` … `if_color` `if_nocolor` | style the string — [styles](fmt.md#styles) |

`upper`/`lower` are not added — Luau's built-in `string.upper`/
`string.lower` already cover them.
