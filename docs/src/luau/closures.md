# Closures

Closures are anonymous functions written `function(params) ... end`.
Luau has no shorthand syntax for them — the `function` keyword is
always required, even for a one-liner.

```lua
local double = function(x) return x * 2 end
double(21)           -- 42
```

Luau's `table` library has no built-in `map`/`filter`/`reduce` — write
a plain loop with a closure inline:

```lua
local function map(xs, f)
    local out = {}
    for _, x in ipairs(xs) do
        table.insert(out, f(x))
    end
    return out
end

local function filter(xs, pred)
    local out = {}
    for _, x in ipairs(xs) do
        if pred(x) then table.insert(out, x) end
    end
    return out
end

map({1, 2, 3}, function(x) return x * 2 end)                  -- {2, 4, 6}
filter({1, 2, 3}, function(x) return x % 2 == 1 end)          -- {1, 3}
```

## Captures

Closures capture variables from the enclosing scope by reference (a
Luau *upvalue* — mutating a captured local through one closure is
visible to every other closure that captured the same local):

```lua
local prefix = "v"
local tagged = map({"1.0", "2.0"}, function(s) return prefix .. s end)
-- tagged == {"v1.0", "v2.0"}
```

## With `fs`

A typical pass over files filters and transforms with closures — since
there's no built-in `filter`/`map`, a plain loop is usually clearer
than composing the two helpers above:

```lua
local configs = {}
for _, p in ipairs(fs.glob("**/*.toml")) do
    if not string.find(p, "/target/") then
        table.insert(configs, toml.parse(fs.read(p)))
    end
end
```
