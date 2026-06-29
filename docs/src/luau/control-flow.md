# Control flow

## `if` / `elseif` / `else`

`if` is a **statement** in Luau, not an expression — it never
yields a value. Use `and`/`or` for a ternary-style expression (see
[Operators](operators.md)).

```lua
if x > 0 then
    print("positive")
elseif x == 0 then
    print("zero")
else
    print("negative")
end

local label = (verbose) and "loud" or "quiet"
```

## No `switch`

Luau has no `switch`/`match` statement. An `if`/`elseif` chain is the
usual replacement; for a large fixed set of cases, a table lookup reads
better:

```lua
local KIND_BY_EXT = {json = "data", tf = "hcl"}
local kind = KIND_BY_EXT[ext] or "other"
```

## Loops

```lua
-- numeric for: start, stop[, step] (inclusive on both ends)
for i = 0, 4 do print(i) end

-- generic for over an array (stops at the first nil)
for _, x in ipairs({10, 20, 30}) do print(x) end

-- generic for over a map (order unspecified)
for k, v in pairs({a = 1, b = 2}) do print(k, v) end

-- for over fs results — the common ito idiom
for _, f in ipairs(fs.glob("**/*.toml")) do
    local cfg = toml.parse(fs.read(f))
    print(`{f}: {cfg.package.version}`)
end

-- for with index: generic for over ipairs already gives you (index, value)
for i, f in ipairs(fs.list("")) do
    print(`{i}: {f}`)
end

-- while / repeat
local n = 0
while n < 3 do
    n += 1
end

repeat
    n -= 1
until n == 0
```

There is no bare `loop { }`; use `while true do ... end` instead.

## `break`, `continue`, `return`

```lua
for _, f in ipairs(fs.glob("**/*.json")) do
    if string.find(f, "node_modules") then continue end   -- skip
    if string.find(f, "stop%.json") then break end          -- stop the loop
end

local function first_match(items, needle)
    for _, x in ipairs(items) do
        if x == needle then return true end   -- early return
    end
    return false   -- Luau has no implicit last-expression return; be explicit
end
```

A Luau function's last statement is **not** its return value unless you
write `return` — there is no implicit-return-of-the-last-expression
behavior. See [Functions](functions.md).
