# Bootstrap the `ito` family of skills

## Goal
Write skills for the `ito` command line utilities `ied` and `ito`.
The command has extensive built-in help; the skill should distill it
into a dense, skimmable reference.

## Information needed
All content comes from the tool's own help.
- `ito --help` — top-level flags.
- `ito --doc` — lists the available help topics.
- `ito --doc <topic>` — one topic in full.
- `ito --doc changelog` — notable changes per release.
- `ito --version` — the installed version.

Since the CLIs use the same modules the scripting surface is the same with exceptions.

## Steps
1. Run `ito --version` and record the exact version in the skill.
2. Run `ito --help` and `ito --doc` for each binary to map out the cli interface and scope.
3. Run `ito --doc <topic>` for every listed topic and distill them into the skill.
4. Add a "Keeping this skill up to date" section stating: this skill was
   written against the recorded version; `ito --doc changelog` prints notable
   changes per release; to update, compare `ito --version` against the
   recorded version and, if newer, fold the newer changelog entries into the
   skill and bump the recorded version.
