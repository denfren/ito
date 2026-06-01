//! `j2` module: render Jinja2 templates with minijinja.
//!
//! - `j2::template(template_text, data)` renders `template_text` using
//!   `data` (any native Rhai value: maps, arrays, scalars) as the
//!   template context, returning the rendered text. Uses default
//!   options.
//! - `j2::engine()` returns a configurable `J2Engine` handle with
//!   chainable setters, then `.template(template_text, data)` renders
//!   with those options applied.
//!
//! Configurable options (all chainable, returning the same handle):
//!   - `.undefined("lenient" | "chainable" | "strict")` — how undefined
//!     variables behave (default `"lenient"`; `"strict"` raises an
//!     error).
//!   - `.trim_blocks(bool)`, `.lstrip_blocks(bool)`,
//!     `.keep_trailing_newline(bool)` — whitespace control.
//!   - `.syntax(block_start, block_end, var_start, var_end,
//!     comment_start, comment_end)` — custom delimiters.
//!   - `.include_root(path)` — enable `{% include %}`/`{% extends %}`,
//!     loading templates from the given VFS directory. Without this,
//!     includes are disabled. Loading is restricted to the VFS root:
//!     `..` escapes are rejected.
//!
//! Like `fs::read`, includes hit current disk state (not buffered
//! writes).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use minijinja::syntax::SyntaxConfig;
use minijinja::{Environment, UndefinedBehavior, value::Value as JinjaValue};
use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Module};

use crate::scripting::pathmap::PathMapper;
use crate::scripting::vfs::Vfs;

/// Custom delimiter set for `set_syntax`.
#[derive(Clone)]
struct Syntax {
    block: (String, String),
    var: (String, String),
    comment: (String, String),
}

/// Configurable rendering options. Defaults mirror minijinja's defaults.
#[derive(Clone, Default)]
struct Options {
    undefined: UndefinedBehavior,
    trim_blocks: bool,
    lstrip_blocks: bool,
    keep_trailing_newline: bool,
    syntax: Option<Syntax>,
    /// Real (resolved) directory to load includes from, if enabled.
    include_root: Option<PathBuf>,
}

/// Configurable Jinja engine handle. Cheap to clone: clones share the
/// same options buffer, so chaining and aliasing observe the same state.
#[derive(Clone)]
pub struct J2Engine {
    options: Rc<RefCell<Options>>,
    /// VFS used to resolve `.include_root(path)` into a real path.
    vfs: Vfs,
}

fn to_err(msg: impl std::fmt::Display) -> Box<EvalAltResult> {
    msg.to_string().into()
}

fn render(
    opts: &Options,
    mapper: &PathMapper,
    template_text: &str,
    data: Dynamic,
) -> Result<String, Box<EvalAltResult>> {
    let ctx: JinjaValue = rhai::serde::from_dynamic(&data)?;
    let mut env = Environment::new();

    env.set_undefined_behavior(opts.undefined);
    env.set_trim_blocks(opts.trim_blocks);
    env.set_lstrip_blocks(opts.lstrip_blocks);
    env.set_keep_trailing_newline(opts.keep_trailing_newline);

    if let Some(s) = &opts.syntax {
        let syntax = SyntaxConfig::builder()
            .block_delimiters(s.block.0.clone(), s.block.1.clone())
            .variable_delimiters(s.var.0.clone(), s.var.1.clone())
            .comment_delimiters(s.comment.0.clone(), s.comment.1.clone())
            .build()
            .map_err(|e| to_err(format!("j2 syntax error: {e}")))?;
        env.set_syntax(syntax);
    }

    if let Some(root) = &opts.include_root {
        // `root` is the script-visible include dir, already resolved to a
        // real path. Re-resolve each include name relative to it through
        // the cloneable resolver so `..` and symlink escapes are rejected.
        let root = root.clone();
        let mapper = mapper.clone();
        env.set_loader(move |name| {
            // Resolve the requested name under `root`, rejecting any
            // `..` traversal that escapes it.
            use std::path::Component;
            let mut rel = PathBuf::new();
            for comp in std::path::Path::new(name).components() {
                match comp {
                    Component::RootDir | Component::Prefix(_) | Component::CurDir => {}
                    Component::ParentDir => {
                        if !rel.pop() {
                            return Err(minijinja::Error::new(
                                minijinja::ErrorKind::InvalidOperation,
                                format!("j2 include escapes root: {name}"),
                            ));
                        }
                    }
                    Component::Normal(seg) => rel.push(seg),
                }
            }
            let target = root.join(&rel);
            // Reject symlink-based escapes out of the VFS root.
            if !mapper.contained(&target) {
                return Err(minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    format!("j2 include escapes root: {name}"),
                ));
            }
            match std::fs::read_to_string(target) {
                Ok(s) => Ok(Some(s)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    format!("j2 include read error: {e}"),
                )),
            }
        });
    }

    env.render_str(template_text, ctx)
        .map_err(|e| to_err(format!("j2 render error: {e}")))
}

/// Register the `j2` module on `engine`.
pub fn register(engine: &mut Engine, vfs: &Vfs) {
    engine.register_type_with_name::<J2Engine>("J2Engine");

    let mut module = Module::new();

    // j2::template(template_text, data) -> String   (default options)
    let v = vfs.clone();
    module.set_native_fn(
        "template",
        move |template_text: ImmutableString,
              data: Dynamic|
              -> Result<String, Box<EvalAltResult>> {
            render(&Options::default(), &v.mapper(), &template_text, data)
        },
    );

    // j2::engine() -> J2Engine
    let v = vfs.clone();
    module.set_native_fn("engine", move || {
        Ok(J2Engine {
            options: Rc::new(RefCell::new(Options::default())),
            vfs: v.clone(),
        })
    });

    engine.register_static_module("j2", module.into());

    // Chainable setters (each returns the same handle).
    macro_rules! flag_setter {
        ($name:literal, $field:ident) => {
            engine.register_fn($name, |e: &mut J2Engine, yes: bool| {
                e.options.borrow_mut().$field = yes;
                e.clone()
            });
        };
    }
    flag_setter!("trim_blocks", trim_blocks);
    flag_setter!("lstrip_blocks", lstrip_blocks);
    flag_setter!("keep_trailing_newline", keep_trailing_newline);

    // .undefined("lenient" | "chainable" | "strict")
    engine.register_fn(
        "undefined",
        |e: &mut J2Engine, mode: ImmutableString| -> Result<J2Engine, Box<EvalAltResult>> {
            let behavior = match mode.as_str() {
                "lenient" => UndefinedBehavior::Lenient,
                "chainable" => UndefinedBehavior::Chainable,
                "strict" => UndefinedBehavior::Strict,
                other => {
                    return Err(to_err(format!(
                        "j2 undefined: unknown mode {other:?} (expected \"lenient\", \"chainable\", or \"strict\")"
                    )));
                }
            };
            e.options.borrow_mut().undefined = behavior;
            Ok(e.clone())
        },
    );

    engine.register_fn(
        "syntax",
        |e: &mut J2Engine,
         block_start: ImmutableString,
         block_end: ImmutableString,
         var_start: ImmutableString,
         var_end: ImmutableString,
         comment_start: ImmutableString,
         comment_end: ImmutableString| {
            e.options.borrow_mut().syntax = Some(Syntax {
                block: (block_start.to_string(), block_end.to_string()),
                var: (var_start.to_string(), var_end.to_string()),
                comment: (comment_start.to_string(), comment_end.to_string()),
            });
            e.clone()
        },
    );

    engine.register_fn(
        "include_root",
        |e: &mut J2Engine, path: ImmutableString| -> Result<J2Engine, Box<EvalAltResult>> {
            let real = e.vfs.resolve(&path)?;
            e.options.borrow_mut().include_root = Some(real);
            Ok(e.clone())
        },
    );

    // J2Engine.template(template_text, data) -> String
    engine.register_fn(
        "template",
        |e: &mut J2Engine,
         template_text: ImmutableString,
         data: Dynamic|
         -> Result<String, Box<EvalAltResult>> {
            render(&e.options.borrow(), &e.vfs.mapper(), &template_text, data)
        },
    );
}
