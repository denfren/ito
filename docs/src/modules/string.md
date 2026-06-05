# `string` — strings

Strings are Rhai built-ins. ito adds `to_*` and `make_*` variants for
common string transformations:

- **`to_*`** — pure: returns a new string, never mutates the receiver.
- **`make_*`** — mutates the string in place **and** returns the result
  (useful for chaining).

These follow ito's stdlib-wide prefix convention; the builder prefix
`with_*` rounds out the family. See
[Method naming conventions](../conventions.md) for the full picture.

## Literals and interpolation

```rhai
let s = "hello";
let t = `hello ${s}`;     // backtick strings interpolate ${…}
let u = `1 + 2 = ${1 + 2}`;
```

## Built-in methods (standard Rhai)

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
| `s.replace(from, to)` | Return copy with all `<from>` replaced by `<to>`. |

## ito string methods

### Trim

| Method | Effect |
| --- | --- |
| `s.to_trimmed()` | Strip leading and trailing whitespace; return copy. *(ito)* |
| `s.to_trimmed_start()` | Strip leading whitespace; return copy. *(ito)* |
| `s.to_trimmed_end()` | Strip trailing whitespace; return copy. *(ito)* |
| `s.make_trimmed()` | Strip leading and trailing whitespace in place; return `s`. *(ito)* |
| `s.make_trimmed_start()` | Strip leading whitespace in place; return `s`. *(ito)* |
| `s.make_trimmed_end()` | Strip trailing whitespace in place; return `s`. *(ito)* |

### Case

| Method | Effect |
| --- | --- |
| `s.make_upper()` | Convert to uppercase in place; return `s`. *(ito)* |
| `s.make_lower()` | Convert to lowercase in place; return `s`. *(ito)* |

### Modify

| Method | Effect |
| --- | --- |
| `s.to_cleared()` | Return `""`. *(ito)* |
| `s.make_cleared()` | Set `s` to `""`; return `s`. *(ito)* |
| `s.to_truncated(len)` | Return copy keeping the first `<len>` characters. *(ito)* |
| `s.make_truncated(len)` | Truncate in place; return `s`. *(ito)* |
| `s.to_cropped(start)` | Return copy from `<start>` to end (negative counts from end). *(ito)* |
| `s.to_cropped(start, len)` | Return copy of `<len>` characters from `<start>`. *(ito)* |
| `s.make_cropped(start)` | Crop in place; return `s`. *(ito)* |
| `s.make_cropped(start, len)` | Crop in place; return `s`. *(ito)* |
| `s.to_set(index, ch)` | Return copy with character at `<index>` replaced by `<ch>` (negative counts from end). *(ito)* |
| `s.make_set(index, ch)` | Replace character at `<index>` in place; return `s`. *(ito)* |
| `s.to_padded(len, fill)` | Return copy right-padded to `<len>` characters using `<fill>` (char or string). *(ito)* |
| `s.make_padded(len, fill)` | Pad in place; return `s`. *(ito)* |
| `s.to_removed(sub)` | Return copy with all occurrences of `<sub>` (char or string) removed. *(ito)* |
| `s.make_removed(sub)` | Remove all occurrences in place; return `s`. *(ito)* |
| `s.to_replaced(from, to)` | Return copy with all `<from>` replaced by `<to>` (char or string on both sides). *(ito)* |
| `s.make_replaced(from, to)` | Replace all occurrences in place; return `s`. *(ito)* |

## Array method

| Method | Effect |
| --- | --- |
| `arr.join(sep)` | Concatenate string elements separated by `<sep>`; return result. *(ito)* |

## Examples

```rhai
// pure — s is unchanged
let t = "  hello  ".to_trimmed();     // "hello"
let u = "hello world".to_removed('o'); // "hell wrld"

// mutating — s is updated, chaining works
let s = "  hello  ";
s.make_trimmed();                       // s == "hello"

let t = "hello".make_upper();          // t == "HELLO", "hello" variable is mutated

// array join
["a", "b", "c"].join(", ")             // "a, b, c"
```
