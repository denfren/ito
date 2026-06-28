use assert_cmd::Command;
use predicates::prelude::*;

fn ito() -> Command {
    Command::cargo_bin("ito").unwrap()
}

#[test]
fn no_subcommand_prints_help_and_exits_nonzero() {
    ito().assert().failure().stderr(predicate::str::contains("Usage"));
}
