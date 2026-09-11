# Method naming conventions

ito's stdlib follows one consistent convention beyond what Luau itself
provides: **fluent builders use a `with_*` prefix**. Each `with_*`
method mutates the builder and returns the same handle, so a whole
object reads as a single chain, finished by a **terminal** call that is
*not* `with_*` (e.g. `.to_string()`, `.run()`, `.send()`, `.interact()`).

A builder with a uniform set of setters may use unprefixed names instead
(`proc.command(...):arg(...):stdout(...):run()`, `fmt.table.new(...):rows(...):to_string()`,
`ui.prompt.input_builder():prompt(...):interact()`). The shape is the same:
constructor, chainable configuration, terminal. Each module chapter's
`## Conventions` says whether a setter mutates a shared handle (the
common case: `hcl`, `proc`, `j2`, `ui.prompt`, `fmt.table`) or returns a
**new** builder and leaves the base unchanged (`encoding.base64.engine`),
and whether validation happens at the setter or at the terminal.

Luau strings are immutable values, so every ito string function is
already pure — there is nothing to mutate in place. See
[`string`](modules/string.md) for the full set of names.

## `with_*` — fluent builders

Builders assemble an object incrementally. Each `with_*` call applies one
piece of configuration to the builder and returns the same builder, so a
whole object reads as a single chain.

```lua
-- hcl — build a document (terminal: :to_string())
local doc = hcl.builder()
    :with_attribute("region", "us-east-1")
    :with_block(
        hcl.block("resource")
            :with_label("aws_instance"):with_label("web")
            :with_attribute("ami", "ami-123")
    )
fs.write("main.tf", doc:to_string())

-- j2 — configure an engine (terminal: :template(...))
local text = j2.engine()
    :with_undefined("strict")
    :with_trim_blocks(true)
    :template("Hello {{ name }}", {name = "world"})
```

Builders carry shared state, so chaining and aliasing observe the same
object — `local b = hcl.builder(); b:with_attribute(...)` mutates `b`
even though the return value is discarded.

## Module functions vs. methods

A module's own functions are called with `.` (e.g. `fs.read(path)`,
`json.parse(text)`); a value returned by a module — a builder, a
cursor, a task — is called with `:` (e.g. `doc:with_attribute(...)`,
`h:block("resource")`). This is ordinary Luau: `:` is sugar that passes
the value before it as the first argument (`v:f(a)` is exactly
`v.f(v, a)`), while `.` on a module table is a plain field lookup.
