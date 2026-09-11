//! `j2` module: render Jinja2 templates with minijinja.
//!
//! - `j2.template(template_text, data[, opts])` renders `template_text` using
//!   `data` (a table, or `nil` for none) as the context, returning the rendered
//!   text. `opts` is a table of the engine's options (`undefined`,
//!   `trim_blocks`, `lstrip_blocks`, `keep_trailing_newline`, `autoescape`).
//! - `j2.engine()` returns a configurable `J2Engine` handle with
//!   chainable setters, then `:template(template_text, data)` renders
//!   with those options applied, and `:render(name, data)` renders a template
//!   loaded from the include root.
//!
//! Configurable options (all chainable: each `:with_*` setter mutates the
//! handle and returns it):
//!   - `:with_undefined("lenient" | "chainable" | "semi_strict" | "strict")` —
//!     how undefined variables behave (default `"lenient"`).
//!   - `:with_trim_blocks(bool)`, `:with_lstrip_blocks(bool)`,
//!     `:with_keep_trailing_newline(bool)` — whitespace control.
//!   - `:with_autoescape("none" | "html" | "json")` — default `"none"`: output
//!     is text, so nothing is escaped unless asked (by template *name* minijinja
//!     would otherwise HTML-escape any included `*.html`/`*.xml`).
//!   - `:with_syntax(block_start, block_end, var_start, var_end,
//!     comment_start, comment_end)` — custom delimiters, validated here.
//!   - `:with_include_root(path)` — enable `{% include %}`/`{% extends %}`/
//!     `:render`, loading templates from the given directory. Without this,
//!     includes are disabled. Loading is confined to that directory (symlinks
//!     and `..` included) under both the rooted and the host `fs`; the rooted
//!     `fs` additionally stays inside the VFS root.
//!   - `:with_filter(name, fn)`, `:with_test(name, fn)`, `:with_global(name,
//!     value)` — Luau callbacks as filters / tests / globals.
//!
//! Built in beyond minijinja's own filters: `tojson`, `urlencode`, loop controls
//! (`{% break %}`/`{% continue %}`), Python-style methods (`d.items()`,
//! `s.upper()`) via `minijinja-contrib`'s pycompat, and the Ansible-flavoured
//! `to_yaml`, `b64encode`, `b64decode`, `regex_replace`, `basename`, `dirname`,
//! `combine`, `mandatory`, `quote`.
//!
//! Like `fs.read`, includes hit current disk state (not buffered writes).
//!
//! The environment is built once per engine and cached until a setter changes
//! it. Luau callbacks run inside a render, which never holds a borrow of the
//! engine's options, so a callback may itself use the engine.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use minijinja::syntax::SyntaxConfig;
use minijinja::value::Rest;
use minijinja::{AutoEscape, Environment, ErrorKind, UndefinedBehavior, value::Value as JinjaValue};
use mlua::{Function, Lua, LuaSerdeExt, Table, UserData, UserDataMethods, Value};

use crate::scripting::pathmap::PathMapper;
use crate::scripting::vfs::Vfs;

type Result<T> = mlua::Result<T>;
type JResult<T> = std::result::Result<T, minijinja::Error>;

fn to_err(msg: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(msg.to_string())
}

/// A minijinja error with its source context and cause chain.
fn fmt_err(ctx: &str, e: &minijinja::Error) -> mlua::Error {
    let mut msg = format!("{ctx}: {e}");
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        msg.push_str(&format!("\ncaused by: {s}"));
        src = s.source();
    }
    to_err(msg)
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Escape {
    #[default]
    None,
    Html,
    Json,
}

/// A Luau function called from inside a render.
///
/// minijinja wants `Send + Sync` callbacks; a Luau function is neither. The
/// environment holding one lives in a `J2Engine` (an `Rc`, so never off this
/// thread) and renders run synchronously on the calling thread, so the function
/// is only ever touched from the thread that owns the `Lua` state.
struct Cb {
    lua: Lua,
    f: Function,
}
// SAFETY: see the type's doc comment — a `Cb` never leaves its thread.
unsafe impl Send for Cb {}
unsafe impl Sync for Cb {}

impl Cb {
    /// Call with minijinja values, returning one.
    fn call(&self, args: &[JinjaValue]) -> JResult<JinjaValue> {
        let bad = |e: mlua::Error| minijinja::Error::new(ErrorKind::InvalidOperation, e.to_string());
        let mut lua_args = Vec::with_capacity(args.len());
        for a in args {
            lua_args.push(self.lua.to_value(a).map_err(bad)?);
        }
        let ret: Value = self
            .f
            .call(mlua::MultiValue::from_iter(lua_args))
            .map_err(bad)?;
        if ret.is_nil() {
            return Ok(JinjaValue::from(()));
        }
        let json: serde_json::Value = self.lua.from_value(ret).map_err(bad)?;
        Ok(JinjaValue::from_serialize(&json))
    }
}

#[derive(Clone)]
enum Global {
    Data(JinjaValue),
    Func(Arc<Cb>),
}

/// Configurable rendering options. Defaults mirror minijinja's defaults, except
/// that nothing is auto-escaped.
#[derive(Clone, Default)]
struct Options {
    undefined: UndefinedBehavior,
    trim_blocks: bool,
    lstrip_blocks: bool,
    keep_trailing_newline: bool,
    escape: Escape,
    syntax: Option<SyntaxConfig>,
    /// Real (resolved) directory to load includes from, if enabled.
    include_root: Option<PathBuf>,
    filters: Vec<(String, Arc<Cb>)>,
    tests: Vec<(String, Arc<Cb>)>,
    globals: Vec<(String, Global)>,
}

fn parse_undefined(mode: &str) -> Result<UndefinedBehavior> {
    Ok(match mode {
        "lenient" => UndefinedBehavior::Lenient,
        "chainable" => UndefinedBehavior::Chainable,
        "semi_strict" => UndefinedBehavior::SemiStrict,
        "strict" => UndefinedBehavior::Strict,
        other => {
            return Err(to_err(format!(
                "j2 undefined: unknown mode {other:?} (expected \"lenient\", \"chainable\", \"semi_strict\", or \"strict\")"
            )));
        }
    })
}

fn parse_escape(mode: &str) -> Result<Escape> {
    Ok(match mode {
        "none" => Escape::None,
        "html" => Escape::Html,
        "json" => Escape::Json,
        other => {
            return Err(to_err(format!(
                "j2 autoescape: unknown mode {other:?} (expected \"none\", \"html\", or \"json\")"
            )));
        }
    })
}

fn bool_opt(key: &str, v: Value) -> Result<bool> {
    match v {
        Value::Boolean(b) => Ok(b),
        other => Err(to_err(format!(
            "j2 opts.{key} must be a boolean, got {}",
            other.type_name()
        ))),
    }
}

impl Options {
    /// Apply the keys of a `j2.template` `opts` table.
    fn apply_table(&mut self, t: &Table) -> Result<()> {
        for pair in t.clone().pairs::<String, Value>() {
            let (k, v) = pair?;
            match k.as_str() {
                "undefined" | "autoescape" => {
                    let Value::String(s) = &v else {
                        return Err(to_err(format!("j2 opts.{k} must be a string")));
                    };
                    let s = s.to_str()?;
                    if k == "undefined" {
                        self.undefined = parse_undefined(&s)?;
                    } else {
                        self.escape = parse_escape(&s)?;
                    }
                }
                "trim_blocks" => self.trim_blocks = bool_opt(&k, v)?,
                "lstrip_blocks" => self.lstrip_blocks = bool_opt(&k, v)?,
                "keep_trailing_newline" => self.keep_trailing_newline = bool_opt(&k, v)?,
                other => {
                    return Err(to_err(format!(
                        "j2: unknown option {other:?} (expected undefined, autoescape, trim_blocks, lstrip_blocks, keep_trailing_newline)"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Where `:with_include_root(..)` resolves its argument, and what an include
/// name is then confined to.
#[derive(Clone)]
enum Fs {
    /// The rooted VFS (`ied`): the path is script-visible, and both it and
    /// every include stay inside the VFS root.
    Rooted(Vfs),
    /// The host filesystem (`ito`): the path is an ordinary cwd-relative or
    /// absolute one.
    Host,
}

/// Configurable Jinja engine handle. Cheap to clone: clones share the
/// same options buffer, so chaining and aliasing observe the same state.
#[derive(Clone)]
pub struct J2Engine {
    options: Rc<RefCell<Options>>,
    /// The built environment, dropped whenever an option changes.
    env: Rc<RefCell<Option<Rc<Environment<'static>>>>>,
    /// The filesystem includes resolve against, when the host exposed one.
    ///
    /// `None` makes `:with_include_root(..)` fail: an include is a file read, so
    /// it needs the same permission `fs` does. Rendering a template *string*
    /// never touches this.
    fs: Option<Fs>,
}

impl J2Engine {
    /// Mutate the options and invalidate the cached environment.
    fn update(&self, f: impl FnOnce(&mut Options)) -> Result<J2Engine> {
        f(&mut self.options.borrow_mut());
        *self.env.borrow_mut() = None;
        Ok(self.clone())
    }

    /// The environment for the current options, built on first use. No borrow
    /// is held once this returns.
    fn environment(&self, lua: &Lua) -> Result<Rc<Environment<'static>>> {
        if let Some(env) = self.env.borrow().as_ref() {
            return Ok(env.clone());
        }
        let opts = self.options.borrow().clone();
        let env = Rc::new(build_env(lua, &opts, self.confinement())?);
        *self.env.borrow_mut() = Some(env.clone());
        Ok(env)
    }

    /// What an include name is bounded by, `None` when there is no filesystem.
    fn confinement(&self) -> Option<Confine> {
        self.fs.as_ref().map(|fs| match fs {
            Fs::Rooted(vfs) => Confine::Root(vfs.mapper()),
            Fs::Host => Confine::IncludeRoot,
        })
    }
}

impl UserData for J2Engine {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("with_trim_blocks", |_, e, yes: bool| {
            e.update(|o| o.trim_blocks = yes)
        });
        methods.add_method("with_lstrip_blocks", |_, e, yes: bool| {
            e.update(|o| o.lstrip_blocks = yes)
        });
        methods.add_method("with_keep_trailing_newline", |_, e, yes: bool| {
            e.update(|o| o.keep_trailing_newline = yes)
        });
        methods.add_method("with_undefined", |_, e, mode: String| {
            let b = parse_undefined(&mode)?;
            e.update(|o| o.undefined = b)
        });
        methods.add_method("with_autoescape", |_, e, mode: String| {
            let x = parse_escape(&mode)?;
            e.update(|o| o.escape = x)
        });

        // Validated here, so a bad delimiter set fails at the setter.
        methods.add_method(
            "with_syntax",
            |_,
             e,
             (bs, be, vs, ve, cs, ce): (String, String, String, String, String, String)| {
                let syntax = SyntaxConfig::builder()
                    .block_delimiters(bs, be)
                    .variable_delimiters(vs, ve)
                    .comment_delimiters(cs, ce)
                    .build()
                    .map_err(|err| to_err(format!("j2 with_syntax: {err}")))?;
                e.update(|o| o.syntax = Some(syntax))
            },
        );

        // Includes read files, so they need the same permission `fs` does. A host
        // that exposes no filesystem passes no VFS, and there is then no
        // directory this could name -- see `register_without_fs`.
        methods.add_method("with_include_root", |_, e, path: String| {
            let real = match e.fs.as_ref() {
                Some(Fs::Rooted(vfs)) => vfs.resolve(&path)?,
                // Made absolute so the loader's containment check has something
                // stable to compare against.
                Some(Fs::Host) => std::env::current_dir()
                    .map(|cwd| cwd.join(&path))
                    .map_err(|err| to_err(format!("getcwd: {err}")))?,
                None => {
                    return Err(to_err(
                        "j2 includes read files, which this command does not expose",
                    ));
                }
            };
            e.update(|o| o.include_root = Some(real))
        });

        // :with_filter(name, fn(value, ...args) -> value)
        methods.add_method("with_filter", |lua, e, (name, f): (String, Function)| {
            let cb = Arc::new(Cb { lua: lua.clone(), f });
            e.update(|o| o.filters.push((name, cb)))
        });
        // :with_test(name, fn(value, ...args) -> bool)
        methods.add_method("with_test", |lua, e, (name, f): (String, Function)| {
            let cb = Arc::new(Cb { lua: lua.clone(), f });
            e.update(|o| o.tests.push((name, cb)))
        });
        // :with_global(name, value | fn(...) -> value)
        methods.add_method("with_global", |lua, e, (name, v): (String, Value)| {
            let g = match v {
                Value::Function(f) => Global::Func(Arc::new(Cb { lua: lua.clone(), f })),
                other => {
                    let json: serde_json::Value = lua
                        .from_value(other)
                        .map_err(|err| to_err(format!("j2 with_global {name:?}: {err}")))?;
                    Global::Data(JinjaValue::from_serialize(&json))
                }
            };
            e.update(|o| o.globals.push((name, g)))
        });

        // engine:template(template_text, data) -> String
        methods.add_method(
            "template",
            |lua, e, (template_text, data): (Value, Value)| -> Result<String> {
                let text = text_arg("template", &template_text)?;
                let ctx = context(lua, "template", data)?;
                let env = e.environment(lua)?;
                env.render_str(&text, ctx)
                    .map_err(|err| fmt_err("j2.template", &err))
            },
        );

        // engine:render(name, data) -> String   (a template from the include root)
        methods.add_method(
            "render",
            |lua, e, (name, data): (String, Value)| -> Result<String> {
                if e.options.borrow().include_root.is_none() {
                    return Err(to_err(
                        "j2 render: no include root (call :with_include_root first)",
                    ));
                }
                let ctx = context(lua, "render", data)?;
                let env = e.environment(lua)?;
                env.get_template(&name)
                    .and_then(|t| t.render(ctx))
                    .map_err(|err| fmt_err(&format!("j2.render {name:?}"), &err))
            },
        );
    }
}

/// How the include loader bounds a resolved target path.
#[derive(Clone)]
enum Confine {
    /// Inside the VFS root (symlinks accounted for), as well as the include root.
    Root(PathMapper),
    /// Inside the include root only.
    IncludeRoot,
}

impl Confine {
    /// Whether `target`, resolved under `include_root`, may be loaded.
    ///
    /// Both shapes bound the target by the include root, with symlinks
    /// resolved. A target that does not exist passes (the read reports it as
    /// missing); any other canonicalize failure refuses.
    fn allows(&self, include_root: &std::path::Path, target: &std::path::Path) -> bool {
        use std::io::ErrorKind::NotFound;
        let within = match (target.canonicalize(), include_root.canonicalize()) {
            (Ok(t), Ok(r)) => t.starts_with(r),
            (Err(e), _) | (_, Err(e)) => e.kind() == NotFound,
        };
        within
            && match self {
                Self::Root(mapper) => mapper.contained(target),
                Self::IncludeRoot => true,
            }
    }
}

fn text_arg(func: &str, v: &Value) -> Result<String> {
    match v {
        Value::String(s) => s
            .to_str()
            .map(|s| s.to_string())
            .map_err(|_| to_err(format!("j2.{func}: template text must be valid UTF-8"))),
        other => Err(to_err(format!(
            "j2.{func}: template text must be a string, got {}",
            other.type_name()
        ))),
    }
}

/// The render context from `data`: a table (an empty one is an empty mapping),
/// or `nil` for none. Anything else, or a value that cannot be represented (a
/// function, say), raises with the function's name.
fn context(lua: &Lua, func: &str, data: Value) -> Result<JinjaValue> {
    match data {
        Value::Nil => Ok(JinjaValue::from_serialize(serde_json::Map::new())),
        Value::Table(_) => {
            let json: serde_json::Value = lua
                .from_value(data)
                .map_err(|e| to_err(format!("j2.{func}: data: {e}")))?;
            Ok(JinjaValue::from_serialize(&json))
        }
        other => Err(to_err(format!(
            "j2.{func}: data must be a table or nil, got {}",
            other.type_name()
        ))),
    }
}

/// Build the environment for `opts`.
///
/// `confine` is only consulted for an include, which `opts.include_root` gates —
/// and that can only be set where a filesystem exists.
fn build_env(lua: &Lua, opts: &Options, confine: Option<Confine>) -> Result<Environment<'static>> {
    let mut env = Environment::new();

    env.set_undefined_behavior(opts.undefined);
    env.set_trim_blocks(opts.trim_blocks);
    env.set_lstrip_blocks(opts.lstrip_blocks);
    env.set_keep_trailing_newline(opts.keep_trailing_newline);
    if let Some(s) = &opts.syntax {
        env.set_syntax(s.clone());
    }

    let escape = match opts.escape {
        Escape::None => AutoEscape::None,
        Escape::Html => AutoEscape::Html,
        Escape::Json => AutoEscape::Json,
    };
    env.set_auto_escape_callback(move |_| escape);
    env.set_unknown_method_callback(minijinja_contrib::pycompat::unknown_method_callback);
    add_builtin_filters(&mut env);

    if let Some(root) = &opts.include_root {
        let root = root.clone();
        let confine =
            confine.ok_or_else(|| to_err("j2 internal: include root set without a filesystem"))?;
        env.set_loader(move |name| {
            // Resolve the requested name under `root`, rejecting any `..`
            // traversal that escapes it.
            use std::path::Component;
            let escapes = || {
                minijinja::Error::new(
                    ErrorKind::InvalidOperation,
                    format!("j2 include escapes root: {name}"),
                )
            };
            let mut rel = PathBuf::new();
            for comp in std::path::Path::new(name).components() {
                match comp {
                    Component::RootDir | Component::Prefix(_) | Component::CurDir => {}
                    Component::ParentDir => {
                        if !rel.pop() {
                            return Err(escapes());
                        }
                    }
                    Component::Normal(seg) => rel.push(seg),
                }
            }
            let target = root.join(&rel);
            if !confine.allows(&root, &target) {
                return Err(escapes());
            }
            match std::fs::read_to_string(target) {
                Ok(s) => Ok(Some(s)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(minijinja::Error::new(
                    ErrorKind::InvalidOperation,
                    format!("j2 include {name}: read error: {e}"),
                )),
            }
        });
    }

    for (name, cb) in &opts.filters {
        let cb = cb.clone();
        env.add_filter(name.clone(), move |v: JinjaValue, rest: Rest<JinjaValue>| {
            let mut args = vec![v];
            args.extend(rest.iter().cloned());
            cb.call(&args)
        });
    }
    for (name, cb) in &opts.tests {
        let cb = cb.clone();
        env.add_test(name.clone(), move |v: JinjaValue, rest: Rest<JinjaValue>| {
            let mut args = vec![v];
            args.extend(rest.iter().cloned());
            cb.call(&args).map(|r| r.is_true())
        });
    }
    for (name, g) in &opts.globals {
        match g {
            Global::Data(v) => env.add_global(name.clone(), v.clone()),
            Global::Func(cb) => {
                let cb = cb.clone();
                env.add_global(
                    name.clone(),
                    JinjaValue::from_function(move |rest: Rest<JinjaValue>| cb.call(&rest)),
                );
            }
        }
    }
    let _ = lua;
    Ok(env)
}

/// The Ansible-flavoured filters every engine has (all pure).
fn add_builtin_filters(env: &mut Environment<'static>) {
    let bad = |m: String| minijinja::Error::new(ErrorKind::InvalidOperation, m);

    env.add_filter("to_yaml", move |v: JinjaValue| -> JResult<String> {
        let json = serde_json::to_value(&v).map_err(|e| bad(format!("to_yaml: {e}")))?;
        let y: yamlito::Yaml =
            serde_json::from_value(json).map_err(|e| bad(format!("to_yaml: {e}")))?;
        Ok(y.to_yaml_string())
    });
    env.add_filter("b64encode", |s: String| base64_encode(s.as_bytes()));
    env.add_filter("b64decode", move |s: String| -> JResult<String> {
        let bytes = base64_decode(&s).ok_or_else(|| bad("b64decode: invalid base64".into()))?;
        String::from_utf8(bytes).map_err(|_| bad("b64decode: result is not valid UTF-8".into()))
    });
    env.add_filter(
        "regex_replace",
        move |s: String, pattern: String, repl: String| -> JResult<String> {
            let re = regex::Regex::new(&pattern)
                .map_err(|e| bad(format!("regex_replace: invalid pattern {pattern:?}: {e}")))?;
            // Python-style `\1` becomes `${1}`; `$1`/`${name}` work as in `re`.
            let mut rep = String::new();
            let mut chars = repl.chars().peekable();
            while let Some(c) = chars.next() {
                match (c, chars.peek()) {
                    ('\\', Some(d)) if d.is_ascii_digit() => {
                        rep.push_str(&format!("${{{d}}}"));
                        chars.next();
                    }
                    _ => rep.push(c),
                }
            }
            Ok(re.replace_all(&s, rep.as_str()).into_owned())
        },
    );
    env.add_filter("basename", |s: String| {
        std::path::Path::new(&s)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    env.add_filter("dirname", |s: String| {
        std::path::Path::new(&s)
            .parent()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    env.add_filter("quote", |s: String| format!("'{}'", s.replace('\'', r"'\''")));
    env.add_filter(
        "mandatory",
        move |v: JinjaValue, msg: Option<String>| -> JResult<JinjaValue> {
            if v.is_undefined() {
                Err(bad(msg.unwrap_or_else(|| "mandatory variable is undefined".into())))
            } else {
                Ok(v)
            }
        },
    );
    // combine(a, b, ...): a new mapping, later arguments winning on a clash.
    env.add_filter(
        "combine",
        move |a: JinjaValue, rest: Rest<JinjaValue>| -> JResult<JinjaValue> {
            let mut out: Vec<(JinjaValue, JinjaValue)> = Vec::new();
            for m in std::iter::once(&a).chain(rest.iter()) {
                let keys = m
                    .try_iter()
                    .map_err(|_| bad("combine: arguments must be mappings".into()))?;
                for k in keys {
                    let v = m.get_item(&k)?;
                    match out.iter_mut().find(|(ek, _)| *ek == k) {
                        Some(slot) => slot.1 = v,
                        None => out.push((k, v)),
                    }
                }
            }
            Ok(JinjaValue::from_iter(out))
        },
    );
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = B64.iter().position(|&b| b == c)? as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits & 0xff) as u8);
        }
    }
    Some(out)
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

fn new_engine(fs: Option<Fs>) -> J2Engine {
    J2Engine {
        options: Rc::new(RefCell::new(Options::default())),
        env: Rc::new(RefCell::new(None)),
        fs,
    }
}

fn register_with(lua: &Lua, fs: Option<Fs>) -> Result<()> {
    let module = lua.create_table()?;

    // j2.template(template_text, data[, opts]) -> String
    //
    // No include root is ever set, so this never reads a file either way.
    module.set(
        "template",
        lua.create_function(
            move |lua, (template_text, data, opts): (Value, Value, Option<Table>)| -> Result<String> {
                let text = text_arg("template", &template_text)?;
                let ctx = context(lua, "template", data)?;
                let engine = new_engine(None);
                if let Some(t) = &opts {
                    engine.options.borrow_mut().apply_table(t)?;
                }
                let env = engine.environment(lua)?;
                env.render_str(&text, ctx)
                    .map_err(|e| fmt_err("j2.template", &e))
            },
        )?,
    )?;

    // j2.engine() -> J2Engine
    module.set(
        "engine",
        lua.create_function(move |_, ()| -> Result<J2Engine> { Ok(new_engine(fs.clone())) })?,
    )?;

    lua.globals().set("j2", module)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua() -> Lua {
        let lua = Lua::new();
        register_host(&lua).expect("register");
        lua
    }

    fn eval(src: &str) -> String {
        lua().load(src).eval::<String>().unwrap_or_else(|e| panic!("{e}\n{src}"))
    }

    fn eval_err(src: &str) -> String {
        lua().load(src).eval::<String>().expect_err("must raise").to_string()
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ito-j2-test-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn basic_render_and_nil_data() {
        assert_eq!(eval(r#"return j2.template("Hi {{ n }}", {n = "x"})"#), "Hi x");
        assert_eq!(eval(r#"return j2.template("static", nil)"#), "static");
    }

    #[test]
    fn data_is_validated_and_errors_are_prefixed() {
        assert!(eval_err(r#"return j2.template("x", 5)"#).contains("j2.template: data must be a table"));
        assert!(eval_err(r#"return j2.template("x", {f = function() end})"#).contains("j2.template: data:"));
        assert!(eval_err(r#"return j2.template("{{ 1 +", {})"#).contains("j2.template"));
    }

    #[test]
    fn strict_semi_strict_and_template_opts() {
        assert!(eval_err(r#"return j2.template("{{ missing }}", {}, {undefined = "strict"})"#).contains("undefined"));
        assert_eq!(eval(r#"return j2.template("{{ missing }}", {})"#), "");
        assert_eq!(
            eval(r#"return j2.template("{{ m if m is defined else 'd' }}", {}, {undefined = "semi_strict"})"#),
            "d"
        );
        assert!(eval_err(r#"return j2.template("x", {}, {bogus = 1})"#).contains("unknown option"));
    }

    #[test]
    fn loop_controls_json_urlencode_and_pycompat() {
        assert_eq!(
            eval(r#"return j2.template("{% for i in xs %}{% if i == 2 %}{% continue %}{% endif %}{% if i == 4 %}{% break %}{% endif %}{{ i }}{% endfor %}", {xs = {1,2,3,4,5}})"#),
            "13"
        );
        assert_eq!(eval(r#"return j2.template("{{ d|tojson }}", {d = {a = 1}})"#), r#"{"a":1}"#);
        assert_eq!(eval(r#"return j2.template("{{ s|urlencode }}", {s = "a b"})"#), "a%20b");
        assert_eq!(eval(r#"return j2.template("{{ s.upper() }}", {s = "ab"})"#), "AB");
        assert_eq!(
            eval(r#"return j2.template("{% for k, v in d.items() %}{{ k }}={{ v }}{% endfor %}", {d = {a = 1}})"#),
            "a=1"
        );
    }

    #[test]
    fn ansible_filters() {
        assert_eq!(eval(r#"return j2.template("{{ 'hello'|b64encode }}", {})"#), "aGVsbG8=");
        assert_eq!(eval(r#"return j2.template("{{ 'aGVsbG8='|b64decode }}", {})"#), "hello");
        // A Luau long string: Jinja itself unescapes `\\1` to `\1`.
        assert_eq!(eval(r#"return j2.template([[{{ 'a-1'|regex_replace('(\\d)', '<\\1>') }}]], {})"#), "a-<1>");
        assert_eq!(eval(r#"return j2.template("{{ '/a/b.txt'|basename }}{{ '/a/b.txt'|dirname }}", {})"#), "b.txt/a");
        assert_eq!(
            eval(r#"local r = j2.template("{% set c = a|combine(b) %}{{ c.x }}{{ c.y }}", {a = {x = 1, y = 2}, b = {y = 3}}) return r"#),
            "13"
        );
        assert!(eval_err(r#"return j2.template("{{ nope|mandatory }}", {})"#).contains("mandatory"));
        assert_eq!(eval(r#"return j2.template("{{ d|to_yaml }}", {d = {a = 1}})"#).trim(), "a: 1");
        assert_eq!(eval(r#"return j2.template("{{ \"it's\"|quote }}", {})"#), r"'it'\''s'");
    }

    #[test]
    fn filters_tests_and_globals_from_luau() {
        assert_eq!(
            eval(
                r#"return j2.engine()
                    :with_filter("shout", function(s, n) return s:upper() .. string.rep("!", n or 1) end)
                    :with_test("big", function(n) return n > 10 end)
                    :with_global("site", "ito")
                    :with_global("twice", function(x) return x * 2 end)
                    :template("{{ 'a'|shout(2) }} {{ 20 is big }} {{ 5 is big }} {{ site }} {{ twice(4) }}", {})"#
            ),
            "A!! True False ito 8"
        );
    }

    #[test]
    fn syntax_is_validated_at_the_setter() {
        assert!(eval_err(r#"return j2.engine():with_syntax("", "", "", "", "", ""):template("x", {})"#).contains("with_syntax"));
        assert_eq!(
            eval(r##"return j2.engine():with_syntax("<%", "%>", "<<", ">>", "<#", "#>"):template("<< n >>", {n = "x"})"##),
            "x"
        );
    }

    #[test]
    fn includes_are_contained_not_escaped_and_missing_raises() {
        let dir = tmp("includes");
        std::fs::write(dir.join("part.html"), "<b>{{ v }}</b>").unwrap();
        std::fs::write(dir.join("top.txt"), "[{% include 'part.html' %}]").unwrap();
        std::fs::write(dir.parent().unwrap().join("ito-j2-secret.txt"), "SECRET").unwrap();
        let d = dir.to_string_lossy();
        let setup = format!(r#"local e = j2.engine():with_include_root("{d}") "#);
        // No autoescape of an included *.html by name.
        assert_eq!(eval(&format!(r#"{setup} return e:template("{{% include 'part.html' %}}", {{v = "<i>"}})"#)), "<b><i></b>");
        assert_eq!(eval(&format!(r#"{setup} return e:render("top.txt", {{v = "<i>"}})"#)), "[<b><i></b>]");
        // Opt-in escaping.
        assert_eq!(
            eval(&format!(r#"{setup} return e:with_autoescape("html"):render("part.html", {{v = "<i>"}})"#)),
            "<b>&lt;i&gt;</b>"
        );
        // Escapes and missing includes raise.
        assert!(eval_err(&format!(r#"{setup} return e:template("{{% include '../ito-j2-secret.txt' %}}", {{}})"#)).contains("escapes root"));
        assert!(eval_err(&format!(r#"{setup} return e:template("{{% include 'nope.txt' %}}", {{}})"#)).contains("not found"));
        assert_eq!(eval(&format!(r#"{setup} return e:template("{{% include 'nope.txt' ignore missing %}}x", {{}})"#)), "x");
        // render without an include root raises.
        assert!(eval_err(r#"return j2.engine():render("a", {})"#).contains("no include root"));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(std::env::temp_dir().join("ito-j2-secret.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_out_of_the_include_root_is_refused() {
        let dir = tmp("symlink");
        let outside = tmp("symlink-outside");
        std::fs::write(outside.join("s.txt"), "SECRET").unwrap();
        std::os::unix::fs::symlink(outside.join("s.txt"), dir.join("link.txt")).unwrap();
        let d = dir.to_string_lossy();
        assert!(eval_err(&format!(
            r#"return j2.engine():with_include_root("{d}"):template("{{% include 'link.txt' %}}", {{}})"#
        ))
        .contains("escapes root"));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn no_fs_raises_on_include_root_but_renders_strings() {
        let lua = Lua::new();
        register_without_fs(&lua).unwrap();
        assert!(lua.load(r#"return j2.engine():with_include_root(".")"#).eval::<Value>().is_err());
        assert_eq!(lua.load(r#"return j2.template("{{ 1 + 1 }}", {})"#).eval::<String>().unwrap(), "2");
    }

    #[test]
    fn environment_is_cached_until_an_option_changes() {
        let out = eval(
            r#"local e = j2.engine()
               local a = e:template("{{ x }}", {x = 1})
               e:with_global("x", "g")
               return a .. e:template("{{ x }}", {})"#,
        );
        assert_eq!(out, "1g");
    }
}
