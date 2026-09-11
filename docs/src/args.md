# args — passing values to scripts

Everything after the script path on the command line belongs to the
script. The program's own flags come before the path and are parsed as
usual; the part after it is handed through verbatim as the global `argv`.

```sh
ito -v deploy.luau --env prod --dry-run
```

```lua
-- argv == { "--env", "prod", "--dry-run" }
print(#argv)        -- 3
print(argv[1])      -- "--env"
```

| | |
| --- | --- |
| `argv` | An array of strings: the arguments given after the script path. |

Four things to know:

- **`argv[1]` is not the program name.** Unlike a C `argv`, only the
  strings you actually passed appear, so `argv[1]` is the first script
  argument and `#argv` is how many there were.
- **It is always a table.** A run with no script arguments sees an empty
  one, never `nil`, so `#argv` and `ipairs(argv)` work unconditionally.
- **It is read-only.** The sandbox freezes it, so `table.remove(argv, 1)`
  or `argv[1] = ...` raises. To take a subcommand off the front, copy the
  rest (`local rest = table.move(argv, 2, #argv, 1, {})`) and hand that to
  `cli.parse(rest)`.
- **Nothing is interpreted.** No option map, no type coercion, no name
  rules — `--env`, `-x`, `prod` and even the program's own flag
  spellings are all just strings once they follow the script path.
  Parsing them is the script's job, which is what lets a script define
  whatever grammar suits it.

```sh
ito run.luau -v --doc x     # argv == { "-v", "--doc", "x" }
```

## In a shebang

Because the script owns everything after its path, a script run through a
`#!` line owns its whole command line:

```lua
#!/usr/bin/env -S ito
print(table.concat(argv, " "))
```

```sh
./deploy --env prod    # the kernel runs: ito ./deploy --env prod
                       # argv == { "--env", "prod" }
```

Flags written into the shebang itself come before the path the kernel
appends, so they are still the program's: `#!/usr/bin/env -S ito -C /srv`
sets the working directory and leaves `argv` alone.

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
