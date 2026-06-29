//! `iq` — query data. A filter first: text in, text out.
//!
//! ```text
//! iq q.luau                 # no input; the query stands alone
//! iq q.luau data.json       # input = the file's text
//! iq q.luau -               # input = stdin
//! iq - data.json            # script from stdin, input from the file
//! iq -s 'return input' -    # inline script, input from stdin
//! iq q.luau -f              # + fs, rooted at the working directory
//! ```
//!
//! Two capabilities, independently optional: `input` exists when the positional
//! names a source, `fs` exists when `-f` is given. Neither is on by default,
//! because a filter that silently reads the filesystem is not a filter.
//!
//! The script's return value is the output, so there is no `print`.

use clap::CommandFactory;
use ito::cli::{IqCli, ParseWithArgv};
use ito::run::{self, Config, Engine, Outcome, RunError, Startup};
use ito::scripting::Argv;

fn main() -> ! {
    // Split the script's own arguments off at `::`, then let clap have the rest.
    let (cli, argv) = IqCli::parse_with_argv();
    // Logging before any check, so a failure below is reported rather than
    // silently setting an exit code.
    run::init_tracing(cli.common.verbose);
    run::main_with(async || run(cli, argv).await)
}

async fn run(cli: IqCli, argv: Argv) -> Result<Outcome, RunError> {
    // --doc/--completion print and exit.
    run::handle_info_flags(&cli.common, IqCli::command())?;
    // Then the checks clap cannot express (see IqCli::validate).
    cli.validate()?;

    let startup = Startup::begin(cli.common)?;
    let Some(script) = startup.script()? else {
        // Bare invocation: the available scripts have been listed.
        return Ok(Outcome::default());
    };

    // Read the input before building the engine: a missing file should fail
    // before a script gets the chance to do anything.
    let input = startup.input(cli.input.input.as_deref())?;

    // `fs` is opt-in (-f), the ambient state is gone, and the answer travels
    // back as a value rather than through `print` -- see `Config::iq`.
    let engine = Engine::build(&script, Config::iq(cli.filesystem, input, argv))?;
    let answer = engine.eval(&script).await?;
    run::emit_returned(&answer, &script, "iq", cli.quiet)?;

    // Nothing was written, so there is no change to report and nothing to flush.
    Ok(Outcome::default())
}
