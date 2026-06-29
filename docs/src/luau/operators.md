# Operators

Full documentation of the language can be found at
[luau.org/syntax](https://luau.org/syntax). To get familiar with it,
try the [Luau Playground](https://luau.org/demo) — note the playground
does not include `ito`'s provided modules.

Luau's operators cover the usual ground; the notes below are the ones
that come up most in `ito` scripts.

## Arithmetic

```lua
1 + 2        -- 3
7 / 2        -- 3.5  (division always yields a float; there is no integer type)
7 // 2       -- 3    (floor division)
10 % 3       -- 1    (modulo)
2 ^ 10       -- 1024 (power)
-x           -- unary negation
```

Luau has one numeric type (a 64-bit float); there is no separate
integer type, so `/` always yields a float.

## Comparison

```lua
a == b   a ~= b
a <  b   a <= b   a >  b   a >= b
```

Comparisons work across numbers and on strings.

## Logical

```lua
a and b   -- short-circuit and
a or b    -- short-circuit or
not a     -- not
```

`and`/`or` also double as a ternary idiom (Luau's `if` is a statement,
not an expression — see [Control flow](control-flow.md)):

```lua
local label = (x > 0) and "positive" or "negative"
```

## Strings

`..` concatenates; numbers are coerced automatically:

```lua
"foo" .. "bar"      -- "foobar"
"count=" .. 3        -- "count=3"
```

Prefer string interpolation for anything non-trivial:

```lua
local f = "/etc/hosts"
print(`reading {f}`)
```

## Membership

Luau has no `in` operator. Test membership directly:

```lua
string.find("fox", "x") ~= nil     -- true (substring)
table.find({1, 2, 3}, 3) ~= nil    -- true (array element)
({key = 1}).key ~= nil             -- true (map key)
```

## No first-class ranges

Luau has no range *values* (no `0..5`) — a range only exists as the
bounds of a numeric `for`:

```lua
for i = 0, 4 do print(i) end   -- 0,1,2,3,4
```

Functions that need a range take explicit bounds instead: e.g.
`finder:depth(1, 2)` — see [`fs`](../modules/fs.md).
