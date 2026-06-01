//! Built-in scripting help.
//!
//! The help topics are mdbook chapter files under `../docs/src`, embedded
//! at compile time. The same files build the documentation website. A
//! single static registry (`TOPICS`) drives the `ito help` listing,
//! single-topic lookup, and the `ito help all` dump.

use std::io::{IsTerminal, Write};
use std::process::{Command, Stdio};

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Group {
    Module,
    Language,
    Other,
}

pub struct Topic {
    /// CLI key, e.g. "fs".
    pub name: &'static str,
    /// Human title, e.g. "fs — virtual filesystem".
    pub title: &'static str,
    pub group: Group,
    /// The embedded chapter markdown.
    pub body: &'static str,
}

pub static TOPICS: &[Topic] = &[
    Topic {
        name: "fs",
        title: "fs — virtual filesystem",
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
        title: "proc — process execution",
        group: Group::Module,
        body: include_str!("../docs/src/modules/proc.md"),
    },
    Topic {
        name: "ito",
        title: "ito — runtime guard",
        group: Group::Module,
        body: include_str!("../docs/src/modules/ito.md"),
    },
    Topic {
        name: "args",
        title: "args — injected globals",
        group: Group::Other,
        body: include_str!("../docs/src/args.md"),
    },
    Topic {
        name: "operators",
        title: "Operators",
        group: Group::Language,
        body: include_str!("../docs/src/rhai/operators.md"),
    },
    Topic {
        name: "control-flow",
        title: "Control flow",
        group: Group::Language,
        body: include_str!("../docs/src/rhai/control-flow.md"),
    },
    Topic {
        name: "functions",
        title: "Functions",
        group: Group::Language,
        body: include_str!("../docs/src/rhai/functions.md"),
    },
    Topic {
        name: "strings-arrays-maps",
        title: "Strings, arrays, maps",
        group: Group::Language,
        body: include_str!("../docs/src/rhai/strings-arrays-maps.md"),
    },
    Topic {
        name: "closures",
        title: "Closures",
        group: Group::Language,
        body: include_str!("../docs/src/rhai/closures.md"),
    },
    Topic {
        name: "imports",
        title: "Imports",
        group: Group::Language,
        body: include_str!("../docs/src/rhai/imports.md"),
    },
    Topic {
        name: "changelog",
        title: "changelog — notable changes per release",
        group: Group::Other,
        body: include_str!("../CHANGELOG.md"),
    },
];

/// The `ito help` entry point: resolve `topic` to markdown (or the topic
/// listing when `topic` is `None`), then emit it. `plain` forces raw
/// markdown (no ANSI); `no_pager` writes straight to stdout instead of
/// paging.
pub fn doc(topic: Option<&str>, plain: bool, no_pager: bool) -> Result<(), String> {
    let md = match topic {
        Some(name) => topic_markdown(name)?,
        None => list_markdown(),
    };
    emit(&md, plain, no_pager);
    Ok(())
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
        .ok_or_else(|| format!("unknown topic '{name}'; try `ito help`"))
}

/// Build the grouped topic listing as markdown.
fn list_markdown() -> String {
    let mut md = String::from("# ito help topics\n\n");
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
    section(&mut md, "Modules", Group::Module);
    section(&mut md, "Rhai language", Group::Language);
    md.push_str("## Other\n\n");
    for t in TOPICS.iter().filter(|t| t.group == Group::Other) {
        let desc = t.title.split_once(" — ").map(|(_, d)| d).unwrap_or(t.title);
        md.push_str(&format!("* `{}` — {}\n", t.name, desc));
    }
    md.push_str("* `all` — all topics in one output\n\n");
    md.push_str("Print a topic with `ito help <topic>`.\n");
    md.push_str("For the command-line flags themselves, see `ito --help`.\n");
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
