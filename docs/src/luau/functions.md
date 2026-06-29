# Functions

Define functions with `function` / `end`, or `local function name(...)
... end` for a named function you don't want to leak into globals
(prefer `local function` — it also lets the function call itself
recursively by name). There is **no implicit return** of the last
expression — write `return` explicitly.

```lua
local function bump(version)
    local parts = string.split(version, ".")
    local patch = tonumber(parts[3]) + 1
    return `{parts[1]}.{parts[2]}.{patch}`
end

print(bump("1.2.3"))   -- "1.2.4"
```

## Parameters

Parameters are positional and untyped; pass any Luau value. (Luau also
supports optional type annotations — `function f(x: number)` — for
tooling/editor checking; ito does not require or rely on them.)

```lua
local function render(name, data)
    return j2.template(fs.read(name), data)
end
```

## Method-call sugar

`v:f(a, b)` is exactly `v.f(v, a, b)` — the value before the colon
becomes the first argument (`self`). This is why the stdlib builders
chain so naturally (`finder:files():find()`), and you can use it on
your own functions too by attaching them to a table:

```lua
local Greeter = {}
function Greeter:shout(suffix)
    return self.name .. suffix
end

local g = setmetatable({name = "hi"}, {__index = Greeter})
g:shout("!")    -- "hi!"
```

For a plain value like a string, ito's own functions on the `string`
table already work this way — `("hi"):trimmed()` — because Luau strings
have `string` itself as their metatable's `__index`.

## The `main` function

After a script's body has run top to bottom, a global `main` function is
called if one is defined:

```lua
local function helper(x)
    return x * 2
end

function main()
    print(helper(21))
end
```

This is the place for entry-point-only logic. A file that is `require`d
as a module has its body run — definitions, exports — but its `main` is
not called, so putting the work inside `main` keeps a script usable both
ways: run it directly and `main` fires, import it and only the
definitions land.

Defining `main` is optional; a script that does everything at the top
level works exactly the same.
