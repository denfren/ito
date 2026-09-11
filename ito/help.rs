//! Built-in scripting help.
//!
//! The help topics are mdbook chapter files under `../docs/src`, embedded
//! at compile time. The same files build the documentation website. A
//! single static registry (`TOPICS`) drives the `ito --doc` listing,
//! single-topic lookup, and the `ito --doc all` dump.

use std::io::{IsTerminal, Write};
use std::process::{Command, Stdio};

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Group {
    /// The per-program overview chapters (`ied`, `ito`).
    ///
    /// Every program embeds all of them, so the book is the same everywhere;
    /// only which one a bare `--doc` opens with differs.
    Program,
    Module,
    Language,
    Other,
}

pub struct Topic {
    /// CLI key, e.g. "fs".
    pub name: &'static str,
    /// Human title, e.g. "fs — files and directories".
    pub title: &'static str,
    pub group: Group,
    /// The embedded chapter markdown.
    pub body: &'static str,
}

pub static TOPICS: &[Topic] = &[
    Topic {
        name: "ied",
        title: "ied — reproducible file edits",
        group: Group::Program,
        body: include_str!("../docs/src/binaries/ied.md"),
    },
    Topic {
        name: "ito",
        title: "ito — the full toolset",
        group: Group::Program,
        body: include_str!("../docs/src/binaries/ito.md"),
    },
    Topic {
        name: "fmt",
        title: "fmt — colours, text, tables",
        group: Group::Module,
        body: include_str!("../docs/src/modules/fmt.md"),
    },
    Topic {
        name: "string",
        title: "string — strings",
        group: Group::Module,
        body: include_str!("../docs/src/modules/string.md"),
    },
    Topic {
        name: "table",
        title: "table — arrays and objects",
        group: Group::Module,
        body: include_str!("../docs/src/modules/table.md"),
    },
    Topic {
        name: "fs",
        title: "fs — files and directories",
        group: Group::Module,
        body: include_str!("../docs/src/modules/fs.md"),
    },
    Topic {
        name: "path",
        title: "path — path manipulation",
        group: Group::Module,
        body: include_str!("../docs/src/modules/path.md"),
    },
    Topic {
        name: "time",
        title: "time — timestamps and deltas",
        group: Group::Module,
        body: include_str!("../docs/src/modules/time.md"),
    },
    Topic {
        name: "re",
        title: "re — regular expressions",
        group: Group::Module,
        body: include_str!("../docs/src/modules/re.md"),
    },
    Topic {
        name: "json",
        title: "json — JSON",
        group: Group::Module,
        body: include_str!("../docs/src/modules/json.md"),
    },
    Topic {
        name: "toml",
        title: "toml — TOML",
        group: Group::Module,
        body: include_str!("../docs/src/modules/toml.md"),
    },
    Topic {
        name: "yaml",
        title: "yaml — YAML",
        group: Group::Module,
        body: include_str!("../docs/src/modules/yaml.md"),
    },
    Topic {
        name: "hcl",
        title: "hcl — HCL",
        group: Group::Module,
        body: include_str!("../docs/src/modules/hcl.md"),
    },
    Topic {
        name: "j2",
        title: "j2 — Jinja2 templates",
        group: Group::Module,
        body: include_str!("../docs/src/modules/j2.md"),
    },
    Topic {
        name: "proc",
        title: "proc — running commands",
        group: Group::Module,
        body: include_str!("../docs/src/modules/proc.md"),
    },
    Topic {
        name: "redirect",
        title: "redirect — where command output goes",
        group: Group::Module,
        body: include_str!("../docs/src/modules/redirect.md"),
    },
    Topic {
        name: "ssh",
        title: "ssh — running commands remotely",
        group: Group::Module,
        body: include_str!("../docs/src/modules/ssh.md"),
    },
    Topic {
        name: "aws.config",
        title: "aws.config — explicit AWS clients",
        group: Group::Module,
        body: include_str!("../docs/src/modules/aws-config.md"),
    },
    Topic {
        name: "aws.ec2",
        title: "aws.ec2 — AWS EC2",
        group: Group::Module,
        body: include_str!("../docs/src/modules/aws-ec2.md"),
    },
    Topic {
        name: "aws.s3",
        title: "aws.s3 — AWS S3",
        group: Group::Module,
        body: include_str!("../docs/src/modules/aws-s3.md"),
    },
    Topic {
        name: "aws.autoscaling",
        title: "aws.autoscaling — AWS EC2 Auto Scaling",
        group: Group::Module,
        body: include_str!("../docs/src/modules/aws-autoscaling.md"),
    },
    Topic {
        name: "aws.cloudtrail",
        title: "aws.cloudtrail — AWS CloudTrail",
        group: Group::Module,
        body: include_str!("../docs/src/modules/aws-cloudtrail.md"),
    },
    Topic {
        name: "aws.cloudwatch",
        title: "aws.cloudwatch — AWS CloudWatch",
        group: Group::Module,
        body: include_str!("../docs/src/modules/aws-cloudwatch.md"),
    },
    Topic {
        name: "task",
        title: "task — concurrent tasks",
        group: Group::Module,
        body: include_str!("../docs/src/modules/task.md"),
    },
    Topic {
        name: "ui.prompt",
        title: "ui.prompt — interactive prompts",
        group: Group::Module,
        body: include_str!("../docs/src/modules/ui-prompt.md"),
    },
    Topic {
        name: "ui.progress",
        title: "ui.progress — progress bars and spinners",
        group: Group::Module,
        body: include_str!("../docs/src/modules/ui-progress.md"),
    },
    // The `ito` *module* is reached as `ito-module`: the bare name `ito` belongs
    // to the program chapter above, which is what someone typing `--doc ito`
    // is far more likely to want.
    Topic {
        name: "ito-module",
        title: "ito-module — the version guard and slug",
        group: Group::Module,
        body: include_str!("../docs/src/modules/ito.md"),
    },
    Topic {
        name: "hash",
        title: "hash — cryptographic digests",
        group: Group::Module,
        body: include_str!("../docs/src/modules/hash.md"),
    },
    Topic {
        name: "encoding",
        title: "encoding — binary-to-text encodings",
        group: Group::Module,
        body: include_str!("../docs/src/modules/encoding.md"),
    },
    Topic {
        name: "ansible",
        title: "ansible — Ansible Vault",
        group: Group::Module,
        body: include_str!("../docs/src/modules/ansible.md"),
    },
    Topic {
        name: "log",
        title: "log — logging",
        group: Group::Module,
        body: include_str!("../docs/src/modules/log.md"),
    },
    Topic {
        name: "io",
        title: "io — raw stream access",
        group: Group::Module,
        body: include_str!("../docs/src/modules/io.md"),
    },
    Topic {
        name: "cli",
        title: "cli — command-line interfaces",
        group: Group::Module,
        body: include_str!("../docs/src/modules/cli.md"),
    },
    Topic {
        name: "intro",
        title: "intro — what these programs are, and how scripts are run",
        group: Group::Other,
        body: include_str!("../docs/src/intro.md"),
    },
    Topic {
        name: "ito-vs-luau",
        title: "ito vs vanilla Luau",
        group: Group::Other,
        body: include_str!("../docs/src/ito-vs-luau.md"),
    },
    Topic {
        name: "conventions",
        title: "conventions — method naming (with_* builders)",
        group: Group::Other,
        body: include_str!("../docs/src/conventions.md"),
    },
    Topic {
        name: "args",
        title: "args — passing values to scripts (argv)",
        group: Group::Other,
        body: include_str!("../docs/src/args.md"),
    },
    Topic {
        name: "operators",
        title: "Operators",
        group: Group::Language,
        body: include_str!("../docs/src/luau/operators.md"),
    },
    Topic {
        name: "control-flow",
        title: "Control flow",
        group: Group::Language,
        body: include_str!("../docs/src/luau/control-flow.md"),
    },
    Topic {
        name: "functions",
        title: "Functions",
        group: Group::Language,
        body: include_str!("../docs/src/luau/functions.md"),
    },
    Topic {
        name: "closures",
        title: "Closures",
        group: Group::Language,
        body: include_str!("../docs/src/luau/closures.md"),
    },
    Topic {
        name: "require",
        title: "require — module loading",
        group: Group::Language,
        body: include_str!("../docs/src/luau/require.md"),
    },
    Topic {
        name: "changelog",
        title: "changelog — notable changes per release",
        group: Group::Other,
        body: include_str!("../CHANGELOG.md"),
    },
];

/// The `--doc` entry point: resolve `topic` to markdown, then emit it.
///
/// With no topic this opens with `program`'s own chapter, followed by the topic
/// listing — someone reaching for `--doc` wants to know what this program does
/// before they want an index of everything. Every program embeds every chapter,
/// so only the starting page differs.
///
/// `plain` forces raw markdown (no ANSI); `no_pager` writes straight to stdout
/// instead of paging.
pub fn doc(program: &str, topic: Option<&str>, plain: bool, no_pager: bool) -> Result<(), String> {
    let md = match topic {
        Some(name) => topic_markdown(name)?,
        None => default_markdown(program),
    };
    emit(&md, plain, no_pager);
    Ok(())
}

/// The bare-`--doc` page: the program's own chapter, then the topic listing.
///
/// Falls back to the listing alone if the program has no chapter, so an
/// unexpected `argv[0]` degrades rather than failing.
fn default_markdown(program: &str) -> String {
    match TOPICS
        .iter()
        .find(|t| t.group == Group::Program && t.name == program)
    {
        Some(chapter) => format!("{}\n\n---\n\n{}", chapter.body.trim_end(), list_markdown()),
        None => list_markdown(),
    }
}

/// Resolve a topic name to its markdown body. `all` concatenates every
/// topic.
fn topic_markdown(name: &str) -> Result<String, String> {
    if name == "all" {
        return Ok(TOPICS
            .iter()
            .map(|t| t.body.trim_end())
            .collect::<Vec<_>>()
            .join("\n\n---\n\n"));
    }
    TOPICS
        .iter()
        .find(|t| t.name == name)
        .map(|t| t.body.to_string())
        .ok_or_else(|| format!("unknown topic '{name}'; try `ito --doc`"))
}

/// Build the grouped topic listing as markdown.
fn list_markdown() -> String {
    let mut md = String::from("# Documentation topics\n\n");
    let section = |md: &mut String, heading: &str, group: Group| {
        md.push_str(&format!("## {heading}\n\n"));
        for t in TOPICS.iter().filter(|t| t.group == group) {
            // Titles are "name — description"; show the name as code and
            // the description, avoiding the doubled name.
            let desc = t.title.split_once(" — ").map(|(_, d)| d).unwrap_or(t.title);
            md.push_str(&format!("* `{}` — {}\n", t.name, desc));
        }
        md.push('\n');
    };
    section(&mut md, "Programs", Group::Program);
    section(&mut md, "Modules", Group::Module);
    section(&mut md, "Luau language", Group::Language);
    md.push_str("## Other\n\n");
    for t in TOPICS.iter().filter(|t| t.group == Group::Other) {
        let desc = t.title.split_once(" — ").map(|(_, d)| d).unwrap_or(t.title);
        md.push_str(&format!("* `{}` — {}\n", t.name, desc));
    }
    md.push_str("* `all` — all topics in one output\n\n");
    md.push_str("Print a topic with `--doc <topic>`.\n");
    md.push_str("For the command-line flags themselves, see `--help`.\n");
    md
}

/// Whether ANSI styling should be suppressed: explicit `--plain`, the
/// `NO_COLOR` convention, or a non-terminal stdout.
fn plain_output(plain: bool) -> bool {
    plain || std::env::var_os("NO_COLOR").is_some() || !std::io::stdout().is_terminal()
}

/// Wrap width for styled rendering: the terminal width, capped at 100 so
/// prose stays readable on very wide terminals.
fn render_width() -> usize {
    let (cols, _) = termimad::terminal_size();
    (cols as usize).clamp(20, 100)
}

/// Emit markdown: raw when [`plain_output`] holds, otherwise styled and
/// (unless `no_pager`) routed through a pager.
fn emit(md: &str, plain: bool, no_pager: bool) {
    if plain_output(plain) {
        print!("{md}");
        if !md.ends_with('\n') {
            println!();
        }
        return;
    }

    let mut text = termimad::text(md);
    text.set_rendering_width(render_width());
    let styled = text.to_string();

    if no_pager || !page(&styled) {
        print!("{styled}");
    }
}

/// Pipe `content` through a pager. Returns `false` if no pager could be
/// spawned (the caller then writes to stdout directly).
///
/// Pager precedence: `$PAGER`, else `less`. When the resolved pager is
/// `less` and `$LESS` is unset, `LESS=FRX` is injected so ANSI styling
/// passes through (`R`), short output is not paged (`F`), and the screen
/// is not cleared on exit (`X`).
fn page(content: &str) -> bool {
    let pager = std::env::var("PAGER")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| "less".to_string());

    // Split into program + args so `PAGER="less -R"` works.
    let mut parts = pager.split_whitespace();
    let Some(program) = parts.next() else {
        return false;
    };
    let mut cmd = Command::new(program);
    cmd.args(parts).stdin(Stdio::piped());

    let is_less = std::path::Path::new(program)
        .file_stem()
        .is_some_and(|s| s == "less");
    if is_less && std::env::var_os("LESS").is_none() {
        cmd.env("LESS", "FRX");
    }

    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(content.as_bytes());
    }
    let _ = child.wait();
    true
}
