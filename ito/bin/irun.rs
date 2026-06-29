//! `irun` — `ito`, with the whole command line handed to the script.
//!
//! Same runtime, same standard library, same trust model as `ito`. The only
//! difference is who owns the arguments: `irun` claims exactly one, the script
//! path, and everything after it reaches the script verbatim as `argv`.
//!
//! That exists for one reason. In a shebang, `ito` and the script's arguments
//! share a command line, and `ito` parses first:
//!
//! ```text
//! #!/usr/bin/env -S ito
//! $ ./deploy.luau --env prod     # error: unknown flag --env
//! $ ./deploy.luau :: --env prod  # works, but every caller must remember `::`
//! ```
//!
//! `irun` has no flags to collide with, so a script is a program whose
//! interface is its own:
//!
//! ```text
//! #!/usr/bin/env -S irun
//! $ ./deploy.luau --env prod -C x   # argv = {"--env", "prod", "-C", "x"}
//! ```
//!
//! There is deliberately **no `-C`**, and no `--doc`, `-v`, `-s` or `-q`
//! either. Intercepting even one would mean a script could never own that
//! spelling: `./backup.luau -C /srv` has to reach the script, or `irun` has
//! failed at the only thing it does. Use `ito` when you want the flags; the two
//! binaries are otherwise the same program.
//!
//! The escape hatches for what `-C` and `-v` would have done are in the script:
//! `fs`/`path` take any path the script computes, and `log` writes to stderr
//! regardless.

use ito::cli::CommonArgs;
use ito::run::{self, Config, Engine, Outcome, RunError, Startup};
use ito::scripting::Argv;

fn main() -> ! {
    // No clap: the command line is not ours to parse. `args_os` order is the
    // whole grammar -- argv[1] is the script, the rest is the script's.
    let mut args = std::env::args();
    let program = args.next();
    let script = args.next();
    let rest: Vec<String> = args.collect();

    // Logging is off (there is no `-v` to turn it on), but `init_tracing` also
    // installs the subscriber that `log` and every internal `tracing::error!`
    // write through -- including the one `main_with` uses to report a failure.
    run::init_tracing(0);
    run::main_with(async move || run(program, script, rest).await)
}

async fn run(
    program: Option<String>,
    script: Option<String>,
    rest: Vec<String>,
) -> Result<Outcome, RunError> {
    let Some(path) = script else {
        // A bare `irun` cannot list the available scripts the way the others do:
        // that listing exists to answer "what can I run?", and someone reaching
        // for `irun` already has a specific file in hand -- it is the shebang
        // line of one. So this is a usage error, not a menu.
        let program = program.as_deref().unwrap_or("irun");
        return Err(format!(
            "usage: {program} <script> [<args>...]\n\n\
             Runs <script> with the full ito standard library. Every argument \
             after it belongs to the script, reaching it as `argv`.\n\n\
             `{program}` itself takes no options -- that is the point of it. Use \
             `ito` for -C, -v, -s, --doc and the rest."
        )
        .into());
    };

    // Reuse `ito`'s startup by describing this invocation in its terms: one
    // script, no flags. Going through `CommonArgs` rather than a second path
    // into `Startup` keeps script resolution (and its shebang handling) shared
    // -- there is one implementation of "what does this path mean", not two.
    let startup = Startup::begin(CommonArgs::script_only(path))?;
    // `script` returns `None` only for a missing `SCRIPT`, which is handled
    // above; a path that does not exist is an error from in there.
    let Some(script) = startup.script()? else {
        return Ok(Outcome::default());
    };

    // Literally `ito`'s engine: same config constructor, so the two cannot
    // drift. There is no `-q` to quiet it and no `INPUT` positional to read --
    // both would be arguments, and `irun` takes none.
    let engine = Engine::build(&script, Config::ito(false, None, Argv(rest)))?;

    engine.eval(&script).await?;
    Ok(Outcome::default())
}
