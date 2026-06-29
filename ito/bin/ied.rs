//! `ied` — edit structured files in a sandbox. No access to the host.
//!
//! Writes are buffered and only reach disk with `--write`, which is what makes
//! the whole change set previewable (`--diff`, `--list-changed`) and a bare run
//! safe: by default the edits are computed, reported on, and dropped.

use clap::CommandFactory;
use ito::cli::{IedCli, ParseWithArgv};
use ito::run::{self, Config, Engine, Outcome, RunError, Startup};
use ito::scripting::Argv;

fn main() -> ! {
    let (cli, argv) = IedCli::parse_with_argv();
    run::init_tracing(cli.common.verbose);
    run::main_with(async || run(cli, argv).await)
}

async fn run(cli: IedCli, argv: Argv) -> Result<Outcome, RunError> {
    run::handle_info_flags(&cli.common, IedCli::command())?;
    cli.validate()?;

    let startup = Startup::begin(cli.common)?;
    let Some(script) = startup.script()? else {
        return Ok(Outcome::default());
    };

    // Read the input before building the engine: a missing file should fail
    // before a script gets the chance to buffer any edits.
    let input = startup.input(cli.input.input.as_deref())?;

    // Writes buffer, the ambient state is gone so a preview matches the run it
    // previews, and a report owning stdout routes `print` to the log -- see
    // `Config::ied`.
    let engine = Engine::build(&script, Config::ied(cli.quiet, &cli.report, input, argv))?;

    engine.eval(&script).await?;

    // Report first, then flush: both read the same pending set, and reporting
    // has to see it before a flush empties it.
    // `Config::ied` always asks for a rooted `fs`, so a VFS was built.
    let vfs = engine
        .vfs()
        .expect("ied buffers writes, so a VFS was built");
    let outcome = run::emit_reports(vfs, &cli.report)?;
    run::flush_writes(vfs, &cli.report)?;
    Ok(outcome)
}
