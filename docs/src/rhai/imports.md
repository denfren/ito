# Imports

Split a script across files with `import`. A module is just another
`.rhai` file; its functions and constants become available under a
namespace.

```rhai
import "lib" as m;   // loads lib.rhai next to this script
print(m::hello());
```

## Where paths resolve

An `import` path is resolved **relative to the directory of the script
that runs the `import`** — not the working directory.

| Source | Base directory |
| --- | --- |
| `ito run <path>` | the directory containing `<path>` |
| `ito run` (discovered default) | the directory holding the script |
| `ito exec` (inline) | the working directory (the `-C` root) |
| `ito run -` (stdin) | the working directory (the `-C` root) |

The `.rhai` extension is implied: `import "lib"` loads `lib.rhai`.
Relative paths (`import "sub/lib"`, `import "../shared"`) are resolved
against the base directory.

So a nested script imports the module next to *itself*, even if a
same-named module also sits at the root:

```text
proj/
  lib.rhai        // hello() = "root"
  sub/
    lib.rhai      // hello() = "sub"
    s.rhai        // import "lib" as m;  ->  m::hello() == "sub"
```

```console
$ ito run proj/sub/s.rhai -C proj
sub
```

## Nested imports

When an imported module *itself* runs `import`, the inner path resolves
relative to **that module's** directory — the importing file — not the
entry script's directory. Each hop is independent.

```text
proj/
  main.rhai       // import "sub/a"
  b.rhai          // val() = "root-b"   (decoy, never picked)
  sub/
    a.rhai        // import "b"         ->  resolves to proj/sub/b.rhai
    b.rhai        // val() = "sub-b"
```

```console
$ ito run proj/main.rhai -C proj
sub-b
```

`a.rhai` lives in `proj/sub/`, so its `import "b"` finds
`proj/sub/b.rhai`, never the `proj/b.rhai` at the entry script's level.

## Sandboxing

Imports honour the same root confinement as the `fs` module (see the
working directory, `-C`). When the script lives **inside** the `-C`
root, an `import` may not escape that root — a `..` (or symlinked) path
that lands outside is rejected, exactly as `fs::read` would reject it:

```console
$ ito run proj/escape.rhai -C proj   # script does: import "../outside"
ERROR script error in proj/escape.rhai: Module not found: ../outside
```

Inline `exec` and stdin scripts use the root as their base directory, so
they are sandboxed too.

The check applies to **every** hop, not just the first: a nested module
that tries to escape is rejected the same way, even several imports deep.

```text
proj/
  main.rhai       // import "sub/a"
  sub/
    a.rhai        // import "../../escape"   ->  outside proj: rejected
```

```console
$ ito run proj/main.rhai -C proj
ERROR script error in proj/main.rhai: Module not found: ../../escape
  in module 'proj/sub/a.rhai'
```

A script that lives **outside** the `-C` root is not sandboxed: it
resolves imports from its own directory with ordinary file lookup, so it
can still reach its own siblings and parents.

```text
elsewhere/
  lib.rhai        // hello() = "elsewhere-lib"
  s.rhai          // import "lib"  ->  resolves to elsewhere/lib.rhai
```

```console
$ ito run elsewhere/s.rhai -C proj
elsewhere-lib
```
