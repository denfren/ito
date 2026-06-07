# Operators

Full documentation of the language can be found at
[rhai.rs/book](https://rhai.rs/book/language/values-and-types.html) (the
_Scripting Language_ section). To get familiar with it, try the
[Playground](https://rhai.rs/playground/stable/) — note the playground
does not include `ito`'s provided modules.

Rhai's operators cover the usual ground; the notes below are the ones
that come up most in `ito` scripts.

## Arithmetic

```rhai
1 + 2        // 3
7 / 2        // 3   (integer division when both are ints)
7.0 / 2.0    // 3.5
10 % 3       // 1   (modulo)
2 ** 10      // 1024 (power)
-x           // unary negation
```

## Comparison

```rhai
a == b   a != b
a <  b   a <= b   a >  b   a >= b
```

Comparisons work across the numeric types and on strings.

## Logical

```rhai
a && b   // short-circuit and
a || b   // short-circuit or
!a       // not
```

## Strings

`+` concatenates; you can also append non-strings, which are stringified:

```rhai
"foo" + "bar"      // "foobar"
"count=" + 3       // "count=3"
```

Prefer string interpolation for anything non-trivial (see
[Strings, arrays, maps](strings-arrays-maps.md)):

```rhai
let f = "/etc/hosts";
print(`reading ${f}`);
```

## `in` — membership

```rhai
"x" in "fox"            // true (substring)
3 in [1, 2, 3]          // true (array element)
"key" in #{ key: 1 }    // true (map key)
```

## Ranges

Ranges are values you can iterate or pass to APIs such as
`finder.depth(...)`:

```rhai
0..5     // exclusive: 0,1,2,3,4
0..=5    // inclusive: 0,1,2,3,4,5

for i in 0..3 { print(i); }
fs::finder("*.rs").depth(1..=2).find();
```
