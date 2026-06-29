//! `j2` module: render Jinja2 templates with minijinja.
//!
//! - `j2.template(template_text, data)` renders `template_text` using
//!   `data` (any native Lua value: tables, strings, numbers, ...) as the
//!   template context, returning the rendered text. Uses default
//!   options.
//! - `j2.engine()` returns a configurable `J2Engine` handle with
//!   chainable setters, then `:template(template_text, data)` renders
//!   with those options applied.
//!
//! Configurable options (all chainable: each `:with_*` setter mutates the
//! handle and returns it):
//!   - `:with_undefined("lenient" | "chainable" | "strict")` — how
//!     undefined variables behave (default `"lenient"`; `"strict"` raises
//!     an error).
//!   - `:with_trim_blocks(bool)`, `:with_lstrip_blocks(bool)`,
//!     `:with_keep_trailing_newline(bool)` — whitespace control.
//!   - `:with_syntax(block_start, block_end, var_start, var_end,
//!     comment_start, comment_end)` — custom delimiters.
//!   - `:with_include_root(path)` — enable `{% include %}`/`{% extends %}`,
//!     loading templates from the given directory. Without this, includes
//!     are disabled. Loading is confined: under a rooted `fs` (`iq`/`ied`)
//!     to the VFS root, under the host `fs` (`ito`) to the include
//!     directory itself. `..` escapes are rejected either way.
//!
//! Like `fs.read`, includes hit current disk state (not buffered writes).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use minijinja::syntax::SyntaxConfig;
use minijinja::{Environment, UndefinedBehavior, value::Value as JinjaValue};
use mlua::{Lua, LuaSerdeExt, UserData, UserDataMethods, Value};

use crate::scripting::pathmap::PathMapper;
use crate::scripting::vfs::Vfs;

type Result<T> = mlua::Result<T>;

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

/// Where `:with_include_root(..)` resolves its argument, and what an include
/// name is then confined to.
#[derive(Clone)]
enum Fs {
    /// The rooted VFS (`iq`/`ied`): the path is script-visible, and both it and
    /// every include stay inside the VFS root.
    Rooted(Vfs),
    /// The host filesystem (`ito`): the path is an ordinary cwd-relative or
    /// absolute one, and includes are confined to *it* rather than to a root
    /// there is none of.
    Host,
}

/// Configurable Jinja engine handle. Cheap to clone: clones share the
/// same options buffer, so chaining and aliasing observe the same state.
#[derive(Clone)]
pub struct J2Engine {
    options: Rc<RefCell<Options>>,
    /// The filesystem includes resolve against, when the host exposed one.
    ///
    /// `None` makes `:with_include_root(..)` fail: an include is a file read, so
    /// it needs the same permission `fs` does. Rendering a template *string*
    /// never touches this.
    fs: Option<Fs>,
}

impl UserData for J2Engine {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        add_option_methods(methods);

        // Includes read files, so they need the same permission `fs` does. A host
        // that exposes no filesystem passes no VFS, and there is then no
        // directory this could name -- see `register_without_fs`.
        methods.add_method_mut(
            "with_include_root",
            |_, e, path: String| -> Result<J2Engine> {
                let real = match e.fs.as_ref() {
                    Some(Fs::Rooted(vfs)) => vfs.resolve(&path)?,
                    // No root to resolve against: an ordinary path, made
                    // absolute so the loader's containment check has something
                    // stable to compare against.
                    Some(Fs::Host) => std::env::current_dir()
                        .map(|cwd| cwd.join(&path))
                        .map_err(|e| to_err(format!("getcwd: {e}")))?,
                    None => {
                        return Err(to_err(
                            "j2 includes read files, which this command does not expose",
                        ));
                    }
                };
                e.options.borrow_mut().include_root = Some(real);
                Ok(e.clone())
            },
        );

        // engine:template(template_text, data) -> String
        methods.add_method(
            "template",
            |lua, e, (template_text, data): (String, Value)| -> Result<String> {
                render(
                    lua,
                    &e.options.borrow(),
                    e.confinement(),
                    &template_text,
                    data,
                )
            },
        );
    }
}

impl J2Engine {
    /// What an include name is bounded by, `None` when there is no filesystem.
    ///
    /// Without one no include root can be set, so nothing is ever loaded
    /// through it.
    fn confinement(&self) -> Option<Confine> {
        self.fs.as_ref().map(|fs| match fs {
            Fs::Rooted(vfs) => Confine::Root(vfs.mapper()),
            Fs::Host => Confine::IncludeRoot,
        })
    }
}

/// How the include loader bounds a resolved target path.
#[derive(Clone)]
enum Confine {
    /// Inside the VFS root, symlinks accounted for.
    Root(PathMapper),
    /// Inside the include root itself — the only boundary available when the
    /// filesystem has no root of its own, and the one the caller named.
    IncludeRoot,
}

impl Confine {
    /// Whether `target`, resolved under `include_root`, may be loaded.
    fn allows(&self, include_root: &std::path::Path, target: &std::path::Path) -> bool {
        match self {
            Self::Root(mapper) => mapper.contained(target),
            // Canonicalize both ends so a symlink out of the include root is
            // caught the same way `PathMapper::contained` catches one out of a
            // VFS root. A target that does not exist is left to the read below
            // to report as missing.
            Self::IncludeRoot => match (target.canonicalize(), include_root.canonicalize()) {
                (Ok(t), Ok(r)) => t.starts_with(r),
                _ => true,
            },
        }
    }
}

fn to_err(msg: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(msg.to_string())
}

/// Render `template_text`.
///
/// `confine` is only consulted for an include, which `opts.include_root` gates —
/// and that can only be set where a filesystem exists, so `None` is unreachable
/// in the branch below rather than a case to handle.
fn render(
    lua: &Lua,
    opts: &Options,
    confine: Option<Confine>,
    template_text: &str,
    data: Value,
) -> Result<String> {
    let json: serde_json::Value = lua.from_value(data)?;
    let ctx = JinjaValue::from_serialize(&json);
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
        // An include root can only have been set where a filesystem exists.
        let confine =
            confine.ok_or_else(|| to_err("j2 internal: include root set without a filesystem"))?;
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
            // Reject symlink-based escapes past the boundary.
            if !confine.allows(&root, &target) {
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

/// The option setters, which need no filesystem and so are always available.
fn add_option_methods<M: UserDataMethods<J2Engine>>(methods: &mut M) {
    methods.add_method_mut("with_trim_blocks", |_, e, yes: bool| {
        e.options.borrow_mut().trim_blocks = yes;
        Ok(e.clone())
    });
    methods.add_method_mut("with_lstrip_blocks", |_, e, yes: bool| {
        e.options.borrow_mut().lstrip_blocks = yes;
        Ok(e.clone())
    });
    methods.add_method_mut("with_keep_trailing_newline", |_, e, yes: bool| {
        e.options.borrow_mut().keep_trailing_newline = yes;
        Ok(e.clone())
    });

    // :with_undefined("lenient" | "chainable" | "strict")
    methods.add_method_mut("with_undefined", |_, e, mode: String| -> Result<J2Engine> {
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
    });

    methods.add_method_mut(
        "with_syntax",
        |_,
         e,
         (block_start, block_end, var_start, var_end, comment_start, comment_end): (
            String,
            String,
            String,
            String,
            String,
            String,
        )| {
            e.options.borrow_mut().syntax = Some(Syntax {
                block: (block_start, block_end),
                var: (var_start, var_end),
                comment: (comment_start, comment_end),
            });
            Ok(e.clone())
        },
    );
}

/// Register the `j2` module on `lua`, with includes reading from `vfs`.
pub fn register(lua: &Lua, vfs: &Vfs) -> Result<()> {
    register_with(lua, Some(Fs::Rooted(vfs.clone())))
}

/// Register the `j2` module on `lua`, with includes reading the host
/// filesystem — the `ito` shape, matching `rfs`.
///
/// `:with_include_root(..)` takes an ordinary path (relative to the work
/// directory, or absolute) rather than a VFS-relative one, and an include name
/// is confined to *that* directory. There is no wider root to fall back on, and
/// letting `{% include "../../etc/passwd" %}` out of the directory the caller
/// explicitly named would make the argument meaningless.
pub fn register_host(lua: &Lua) -> Result<()> {
    register_with(lua, Some(Fs::Host))
}

/// Register the `j2` module without a filesystem.
///
/// Rendering a template *string* needs none, so `j2` stays fully usable; only
/// `:with_include_root(..)` is absent, because an include is a file read. For a
/// host that exposes no `fs`, that closes the gap — `fs` being nil is not much of
/// a boundary if a template can still read the disk.
pub fn register_without_fs(lua: &Lua) -> Result<()> {
    register_with(lua, None)
}

fn register_with(lua: &Lua, fs: Option<Fs>) -> Result<()> {
    let module = lua.create_table()?;

    // j2.template(template_text, data) -> String   (default options)
    //
    // Default options mean no include root, so this never reads a file either way.
    module.set(
        "template",
        lua.create_function(
            move |lua, (template_text, data): (String, Value)| -> Result<String> {
                render(lua, &Options::default(), None, &template_text, data)
            },
        )?,
    )?;

    // j2.engine() -> J2Engine
    module.set(
        "engine",
        lua.create_function(move |_, ()| -> Result<J2Engine> {
            Ok(J2Engine {
                options: Rc::new(RefCell::new(Options::default())),
                fs: fs.clone(),
            })
        })?,
    )?;

    lua.globals().set("j2", module)?;
    Ok(())
}
