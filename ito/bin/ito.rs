//! `ito` — the full toolset: direct writes plus commands, SSH, EC2 and tasks.
//!
//! Writes land as they happen. This binary can spawn subprocesses, so buffering
//! would promise an atomicity that a single `process` call breaks — and
//! `--dry-run` would promise a restraint the script controls, not us. Neither is
//! offered; `ied` is where previewable edits live.

use clap::CommandFactory;
use ito::cli::{ItoCli, ParseWithArgv};
use ito::run::{self, Config, Engine, Outcome, RunError, Startup};
use ito::scripting::Argv;

fn main() -> ! {
    let (cli, argv) = ItoCli::parse_with_argv();
    run::init_tracing(cli.common.verbose);
    run::main_with(async || run(cli, argv).await)
}

async fn run(cli: ItoCli, argv: Argv) -> Result<Outcome, RunError> {
    run::handle_info_flags(&cli.common, ItoCli::command())?;

    // Like --doc, this answers a question about the tool and stops.
    if cli.ai_skill_init {
        print!(include_str!("../ai-skill-bootstrap.md"));
        return Ok(Outcome::default());
    }

    cli.validate()?;

    let startup = Startup::begin(cli.common)?;
    let Some(script) = startup.script()? else {
        return Ok(Outcome::default());
    };

    // Read the input before building the engine: a missing file should fail
    // before a script gets the chance to run a command or write anything.
    let input = startup.input(cli.input.input.as_deref())?;

    // Writes land immediately (the script reads back its own, and a subprocess
    // sees them), plus the surface that reaches outside the sandbox and `task`
    // to overlap it -- see `Config::ito`.
    let engine = Engine::build(&script, Config::ito(cli.quiet, input, argv))?;

    // `eval` drains outstanding tasks, so a task spawned purely for its writes
    // has finished by the time this returns. Nothing to flush: every write
    // already landed.
    engine.eval(&script).await?;
    Ok(Outcome::default())
}
