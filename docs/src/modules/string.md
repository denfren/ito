# `string` — strings

Strings are Rhai built-ins. ito patches the mutating methods so they
also **return the resulting string**, enabling assignment and chaining
while still mutating in place. Methods marked *(ito)* are new or
patched; the rest are standard Rhai.

## Literals and interpolation

```rhai
let s = "hello";
let t = `hello ${s}`;     // backtick strings interpolate ${…}
let u = `1 + 2 = ${1 + 2}`;
```

## Common built-in methods

| Method | Effect |
| --- | --- |
| `s.len()` | Character count. |
| `s.is_empty()` | `true` if the string is `""`. |
| `s.contains(sub)` | `true` if `<sub>` (string or char) is present. |
| `s.starts_with(pre)` | `true` if string starts with `<pre>`. |
| `s.ends_with(suf)` | `true` if string ends with `<suf>`. |
| `s.to_upper()` | Return uppercase copy (does not mutate). |
| `s.to_lower()` | Return lowercase copy (does not mutate). |
| `s.index_of(sub)` | Index of first occurrence, or `-1`. |
| `s.sub_string(start, len)` | Substring copy. |
| `s.split(sep)` | Split on separator; returns array of strings. |

## ito-patched mutating methods

These mutate `s` in place **and** return the result (Rhai's originals
return unit).

| Method | Effect |
| --- | --- |
| `s.trim()` | Strip leading and trailing whitespace. *(ito)* |
| `s.trim_start()` | Strip leading whitespace only. *(ito)* |
| `s.trim_end()` | Strip trailing whitespace only. *(ito)* |
| `s.make_upper()` | Convert to uppercase in place. *(ito)* |
| `s.make_lower()` | Convert to lowercase in place. *(ito)* |
| `s.clear()` | Set to `""`. *(ito)* |
| `s.truncate(len)` | Keep the first `<len>` characters. *(ito)* |
| `s.crop(start)` | Keep from `<start>` to end (negative counts from end). *(ito)* |
| `s.crop(start, len)` | Keep `<len>` characters from `<start>`. *(ito)* |
| `s.set(index, ch)` | Replace character at `<index>` with `<ch>` (negative counts from end). *(ito)* |
| `s.pad(len, fill)` | Right-pad to `<len>` characters using `<fill>` (char or string). *(ito)* |
| `s.remove(sub)` | Remove all occurrences of `<sub>` (char or string). *(ito)* |
| `s.replace(from, to)` | Replace all occurrences of `<from>` with `<to>` (char or string on both sides). *(ito)* |

```rhai
"  hello  ".trim()         // "hello"
"  hello  ".trim_start()   // "hello  "
"  hello  ".trim_end()     // "  hello"

let s = "  hello  ";
let t = s.trim();          // s is mutated; t == "hello"
```
