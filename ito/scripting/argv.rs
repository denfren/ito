//! The `argv` global: the script's own arguments.
//!
//! Everything after a bare `::` on the command line belongs to the script, not
//! to the binary:
//!
//! ```text
//! ito deploy.luau -v :: --env prod --dry-run
//! ```
//!
//! The part before `::` is parsed by clap as usual; the part after is passed
//! through verbatim as a list of strings ([`Argv::split`]) and registered here
//! as the global `argv`.
//!
//! Unlike a C `argv`, **element 1 is not the program name** — only the strings
//! given after `::` appear, so `argv[1]` is the script's first argument and
//! `#argv` is how many were passed. A script that got none sees an empty table,
//! never nil.
//!
//! Parsing them is the script's job: there is no option map, no type coercion
//! and no name validation, because every script wants a different grammar and
//! guessing one only gets in the way.

/// The script's arguments, split off the raw command line.
#[derive(Debug, Clone, Default)]
pub struct Argv(pub Vec<String>);

impl Argv {
    /// Partition `args` at the first bare `::`.
    ///
    /// Returns the script's arguments and the remainder for clap. Only the
    /// *first* `::` separates: later ones are ordinary script arguments, since
    /// the script's grammar is its own business.
    pub fn split(args: &[String]) -> (Self, Vec<String>) {
        match args.iter().position(|a| a == "::") {
            Some(i) => (Self(args[i + 1..].to_vec()), args[..i].to_vec()),
            None => (Self::default(), args.to_vec()),
        }
    }
}

/// Register the `argv` global on `lua`.
pub fn register(lua: &mlua::Lua, argv: Argv) -> mlua::Result<()> {
    lua.globals()
        .set("argv", lua.create_sequence_from(argv.0)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn splits_at_the_separator() {
        let (argv, rest) = Argv::split(&v(&["s.luau", "-v", "::", "--env", "prod"]));
        assert_eq!(argv.0, v(&["--env", "prod"]));
        assert_eq!(rest, v(&["s.luau", "-v"]));
    }

    #[test]
    fn no_separator_leaves_everything_to_clap() {
        let (argv, rest) = Argv::split(&v(&["s.luau", "-v"]));
        assert!(argv.0.is_empty());
        assert_eq!(rest, v(&["s.luau", "-v"]));
    }

    #[test]
    fn trailing_separator_yields_no_arguments() {
        let (argv, rest) = Argv::split(&v(&["s.luau", "::"]));
        assert!(argv.0.is_empty());
        assert_eq!(rest, v(&["s.luau"]));
    }

    #[test]
    fn later_separators_belong_to_the_script() {
        // The script's grammar is its own, so only the first `::` is ours.
        let (argv, rest) = Argv::split(&v(&["s.luau", "::", "a", "::", "b"]));
        assert_eq!(argv.0, v(&["a", "::", "b"]));
        assert_eq!(rest, v(&["s.luau"]));
    }

    #[test]
    fn only_a_bare_separator_counts() {
        // `::x` and `a::b` are values, not the delimiter.
        let (argv, rest) = Argv::split(&v(&["::x", "a::b"]));
        assert!(argv.0.is_empty());
        assert_eq!(rest, v(&["::x", "a::b"]));
    }
}
