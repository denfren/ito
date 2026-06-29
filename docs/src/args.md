# args — passing values to scripts

Everything after a bare `::` on the command line belongs to the script.
The part before it is parsed as usual; the part after is handed through
verbatim as the global `argv`.

```sh
ito deploy.luau -v :: --env prod --dry-run
```

```lua
-- argv == { "--env", "prod", "--dry-run" }
print(#argv)        -- 3
print(argv[1])      -- "--env"
```

| | |
| --- | --- |
| `argv` | An array of strings: the arguments given after `::`. |

Three things to know:

- **`argv[1]` is not the program name.** Unlike a C `argv`, only the
  strings you actually passed appear, so `argv[1]` is the first script
  argument and `#argv` is how many there were.
- **It is always a table.** A run with no `::` sees an empty one, never
  `nil`, so `#argv` and `ipairs(argv)` work unconditionally.
- **Nothing is interpreted.** No option map, no type coercion, no name
  rules — `--env`, `-x`, `prod` and `::` are all just strings. Parsing
  them is the script's job, which is what lets a script define whatever
  grammar suits it.

Only the *first* `::` separates; later ones are ordinary arguments.

```sh
ito run.luau :: a :: b     # argv == { "a", "::", "b" }
```

## Without the separator: `irun`

`::` exists because `ito` and the script share a command line. When the
script should own that line outright — chiefly in a `#!` line —
[`irun`](binaries/irun.md) drops every flag of its own, takes the script
path as its single argument, and hands the rest straight to `argv`:

```sh
irun deploy.luau --env prod    # argv == { "--env", "prod" }
```

`argv` behaves identically either way; only who claimed the arguments
changes. Under `irun` there is nothing to separate, so `::` is not special
and arrives as a plain string like anything else.

## Parsing them: `cli`

You rarely have to read `argv` by hand. The [`cli`](modules/cli.md) module
takes a declaration of what the script accepts and does the rest,
including a generated `--help`:

```lua
local dry   = cli.opt("n/dry-run", "report without writing")
local files = cli.arg("files...",  "files to change")

function main()
    cli.parse()
    if dry() then
        print("dry run — no changes written")
    end
    for _, f in ipairs(files()) do print(f) end
end
```

`cli` is opt-in and takes nothing away: `argv` is still the raw list
beside it, so a script whose grammar `cli` does not cover can parse the
list itself.

## Entry-point-only code

There is no injected "am I the entry script?" constant. Put such code in
a global `main` function instead: it is called after the script body, and
only for the script that was run — a file loaded with `require` has its
body evaluated but its `main` left alone.

```lua
function main()
    print("running as the entry script")
end
```

See [Functions](luau/functions.md#the-main-function).
