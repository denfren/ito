//! `hcl` module: parse HCL into native Lua values, and build HCL
//! documents with a fluent builder.
//!
//! Parsing:
//! - `hcl.parse(text)` deserializes via the `hcl-rs` crate into an
//!   `hcl::Value`, then converts to a Lua value (tables, strings,
//!   numbers, bools, nil), so scripts can inspect HCL content with
//!   ordinary Lua indexing and iteration.
//!
//! Building:
//! - `hcl.builder()` starts a body (the top-level document).
//! - `hcl.block(ident)` starts a block.
//! - Both expose chainable `:with_attribute(key, value)` and
//!   `:with_block(child)`; blocks additionally have `:with_label(text)`.
//!   These `with_*` methods mutate the builder and return it for chaining.
//! - `body:to_string()` renders the document as HCL text.
//!
//! Builders carry shared (`Rc<RefCell<..>>`) state, so they behave like
//! handles: chaining and aliasing both observe the same accumulator.
//! Attribute values accept any Lua value (tables, arrays, scalars) and
//! are converted to HCL expressions.
//!
//! Editing:
//! - `hcl.edit(text)` parses HCL into an editable, format-preserving
//!   handle (`HclDoc`). A cursor/path API navigates and mutates it
//!   in place while keeping layout and comments intact; `:to_string()`
//!   renders the result. See the `edit` submodule.

mod convert;
mod edit;
mod types;
mod visit;

use std::cell::RefCell;
use std::rc::Rc;

use mlua::{Lua, LuaSerdeExt, UserData, UserDataMethods, UserDataRef, Value};

use edit::HclDoc;

type Result<T> = mlua::Result<T>;

fn to_err(msg: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(msg.to_string())
}

/// A raw HCL expression value (bare keyword, traversal, function call,
/// etc.), produced by `hcl.expr`/`hcl.ident`. Unlike a plain string
/// attribute value (which is emitted quoted), this is rendered verbatim.
#[derive(Clone)]
pub struct HclExpr(hcl::Expression);

impl HclExpr {
    /// The wrapped value-model expression. Used by the `hcl.edit` cursor
    /// to bridge an `hcl.expr`/`hcl.ident` value into the edit CST.
    pub(super) fn into_inner(self) -> hcl::Expression {
        self.0
    }

    /// Wrap a value-model expression. Used by the `hcl.edit` cursor's
    /// `read_raw` to hand back a computed expression as an `HclExpr`.
    pub(super) fn from_expression(expr: hcl::Expression) -> Self {
        HclExpr(expr)
    }
}

impl UserData for HclExpr {}

/// In-progress HCL block: an identifier, ordered labels, a body, and an
/// optional leading comment on the block's own header line.
#[derive(Clone)]
pub struct HclBlock {
    ident: String,
    labels: Rc<RefCell<Vec<String>>>,
    body: HclBody,
    /// Leading comment for the block's header (case (b)).
    comment: Rc<RefCell<Option<String>>>,
}

/// A single structure in a body: either a leaf attribute or a nested
/// block, each carrying its own optional leading comment. Blocks keep
/// the full `HclBlock` (not a collapsed `hcl::Block`) so nested comments
/// survive to render time.
#[derive(Clone)]
enum Item {
    Attribute(hcl::Attribute, Option<String>),
    Block(HclBlock),
}

/// In-progress HCL body: an ordered list of items (attributes and nested
/// blocks). The top-level document is a body. `header` is a standalone
/// leading comment at the top of the body, not bound to any item (case
/// (a)).
#[derive(Clone, Default)]
pub struct HclBody {
    items: Rc<RefCell<Vec<Item>>>,
    header: Rc<RefCell<Option<String>>>,
}

/// Convert a Lua value into an HCL expression. A raw `HclExpr` wrapper
/// (from `hcl.expr`/`hcl.ident`) passes through verbatim; anything else
/// goes through serde and is treated as a literal value.
fn to_expr(lua: &Lua, value: Value) -> Result<hcl::Expression> {
    if let Value::UserData(ud) = &value
        && ud.is::<HclExpr>()
    {
        return Ok(ud.borrow::<HclExpr>()?.clone().0);
    }
    let json: serde_json::Value = lua.from_value(value)?;
    hcl::to_expression(json).map_err(|e| to_err(format!("hcl value error: {e}")))
}

fn ident(name: &str) -> Result<hcl::Identifier> {
    hcl::Identifier::new(name).map_err(|e| to_err(format!("hcl identifier error: {e}")))
}

/// Validate a comment string: non-empty, and every line (after optional
/// leading whitespace) must start with an HCL line-comment marker (`#`
/// or `//`). The caller supplies the markers; we never add them.
fn validate_comment(text: &str) -> Result<()> {
    if text.is_empty() {
        return Err(to_err("hcl comment error: empty comment"));
    }
    for line in text.lines() {
        let t = line.trim_start();
        if !(t.starts_with('#') || t.starts_with("//")) {
            return Err(to_err(format!(
                "hcl comment error: every line must start with `#` or `//`: {line:?}"
            )));
        }
    }
    Ok(())
}

/// Indent every non-empty line of `s` by `pad`, preserving blank lines.
fn indent_lines(s: &str, pad: &str) -> String {
    s.lines()
        .map(|l| {
            if l.is_empty() {
                String::new()
            } else {
                format!("{pad}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Emit comment lines at the given indent.
fn render_comment(out: &mut String, comment: &Option<String>, pad: &str) {
    if let Some(c) = comment {
        for line in c.lines() {
            out.push_str(pad);
            out.push_str(line);
            out.push('\n');
        }
    }
}

impl HclBody {
    fn add_attribute(&mut self, lua: &Lua, key: &str, value: Value) -> Result<()> {
        let attr = hcl::Attribute::new(ident(key)?, to_expr(lua, value)?);
        self.items.borrow_mut().push(Item::Attribute(attr, None));
        Ok(())
    }

    fn add_block(&mut self, block: &HclBlock) {
        self.items.borrow_mut().push(Item::Block(block.clone()));
    }

    /// Attach a comment per the placement rules: empty body → set the
    /// standalone `header` (case (a)); otherwise comment the last item
    /// (case (c)).
    fn add_comment(&mut self, text: &str) -> Result<()> {
        validate_comment(text)?;
        let mut items = self.items.borrow_mut();
        match items.last_mut() {
            None => *self.header.borrow_mut() = Some(text.to_string()),
            Some(Item::Attribute(_, c)) => *c = Some(text.to_string()),
            Some(Item::Block(b)) => *b.comment.borrow_mut() = Some(text.to_string()),
        }
        Ok(())
    }

    /// Render the body to HCL text at the given indent depth.
    fn render(&self, depth: usize) -> Result<String> {
        let pad = "  ".repeat(depth);
        let mut out = String::new();
        render_comment(&mut out, &self.header.borrow(), &pad);
        let mut prev: Option<bool> = None; // Some(is_block) of the previous item
        for item in self.items.borrow().iter() {
            let is_block = matches!(item, Item::Block(_));
            // Blank line around blocks: separate a block from whatever
            // precedes it, and from whatever follows it (so attr↔block and
            // block↔block both get one line of distance).
            if let Some(prev_block) = prev {
                if prev_block || is_block {
                    out.push('\n');
                }
            }
            prev = Some(is_block);
            match item {
                Item::Attribute(attr, comment) => {
                    render_comment(&mut out, comment, &pad);
                    let rendered = hcl::format::to_string(attr)
                        .map_err(|e| to_err(format!("hcl serialize error: {e}")))?;
                    out.push_str(&indent_lines(rendered.trim_end(), &pad));
                    out.push('\n');
                }
                Item::Block(block) => {
                    render_comment(&mut out, &block.comment.borrow(), &pad);
                    out.push_str(&block.render_header(&pad)?);
                    out.push_str(&block.body.render(depth + 1)?);
                    out.push_str(&pad);
                    out.push_str("}\n");
                }
            }
        }
        Ok(out)
    }
}

impl HclBlock {
    /// Attach a comment per the placement rules: empty inner body →
    /// comment the block's own header (case (b)); otherwise comment the
    /// last inner item (case (c)).
    fn add_comment(&mut self, text: &str) -> Result<()> {
        validate_comment(text)?;
        if self.body.items.borrow().is_empty() {
            *self.comment.borrow_mut() = Some(text.to_string());
            Ok(())
        } else {
            self.body.add_comment(text)
        }
    }

    /// Render the block's header line (`ident "label"… {`) at `pad`,
    /// validating the identifier and quoting labels.
    fn render_header(&self, pad: &str) -> Result<String> {
        // Validate the identifier (same fallible path as elsewhere).
        let id = ident(&self.ident)?;
        let mut header = format!("{pad}{id}");
        for label in self.labels.borrow().iter() {
            header.push_str(&format!(" {label:?}"));
        }
        header.push_str(" {\n");
        Ok(header)
    }

    /// Render this block to standalone HCL text (header, body, closing
    /// brace) at the given indent `depth`. Used to bridge a builder block
    /// into the `hcl.edit` CST at the right nesting level.
    pub(super) fn render(&self, depth: usize) -> Result<String> {
        let pad = "  ".repeat(depth);
        let mut out = String::new();
        render_comment(&mut out, &self.comment.borrow(), &pad);
        out.push_str(&self.render_header(&pad)?);
        out.push_str(&self.body.render(depth + 1)?);
        out.push_str(&pad);
        out.push_str("}\n");
        Ok(out)
    }

    /// Parse this builder block into an `hcl::edit` CST `Block`, rendered at
    /// indent `depth` so it inserts cleanly into an editable document. The
    /// resulting block's decor already carries the correct indentation.
    pub(super) fn to_edit_block(&self, depth: usize) -> Result<hcl::edit::structure::Block> {
        let text = self.render(depth)?;
        let body: hcl::edit::structure::Body = text
            .parse()
            .map_err(|e| to_err(format!("hcl block build error: {e}")))?;
        body.into_blocks()
            .next()
            .ok_or_else(|| to_err("hcl block build error: no block produced"))
    }
}

impl UserData for HclBody {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut(
            "with_attribute",
            |lua, body, (key, value): (String, Value)| -> Result<HclBody> {
                body.add_attribute(lua, &key, value)?;
                Ok(body.clone())
            },
        );
        methods.add_method_mut(
            "with_block",
            |_, body, block: UserDataRef<HclBlock>| -> Result<HclBody> {
                body.add_block(&block);
                Ok(body.clone())
            },
        );
        methods.add_method_mut("with_comment", |_, body, text: String| -> Result<HclBody> {
            body.add_comment(&text)?;
            Ok(body.clone())
        });
        methods.add_method("to_string", |_, body, ()| body.render(0));
        methods.add_meta_method(mlua::MetaMethod::ToString, |_, body, ()| body.render(0));
    }
}

impl UserData for HclBlock {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("with_label", |_, block, label: String| {
            block.labels.borrow_mut().push(label);
            Ok(block.clone())
        });
        methods.add_method_mut(
            "with_attribute",
            |lua, block, (key, value): (String, Value)| -> Result<HclBlock> {
                block.body.add_attribute(lua, &key, value)?;
                Ok(block.clone())
            },
        );
        methods.add_method_mut(
            "with_block",
            |_, block, child: UserDataRef<HclBlock>| -> Result<HclBlock> {
                block.body.add_block(&child);
                Ok(block.clone())
            },
        );
        methods.add_method_mut(
            "with_comment",
            |_, block, text: String| -> Result<HclBlock> {
                block.add_comment(&text)?;
                Ok(block.clone())
            },
        );
    }
}

/// Register the `hcl` module on `lua`.
pub fn register(lua: &Lua) -> Result<()> {
    let module = lua.create_table()?;

    // hcl.parse(text) -> value
    module.set(
        "parse",
        lua.create_function(|lua, text: String| -> Result<Value> {
            let value: hcl::Value =
                hcl::from_str(&text).map_err(|e| to_err(format!("hcl parse error: {e}")))?;
            lua.to_value(&value)
        })?,
    )?;

    // hcl.to_string(value) -> String
    // Serialize a Lua value (e.g. a parsed/constructed table) to HCL
    // text. Like toml, this does not necessarily round-trip.
    module.set(
        "to_string",
        lua.create_function(|lua, value: Value| -> Result<String> {
            let v: hcl::Value = lua.from_value(value)?;
            hcl::to_string(&v).map_err(|e| to_err(format!("hcl serialize error: {e}")))
        })?,
    )?;

    // hcl.builder() -> HclBody
    module.set(
        "builder",
        lua.create_function(|_, ()| Ok(HclBody::default()))?,
    )?;

    // hcl.block(ident) -> HclBlock
    module.set(
        "block",
        lua.create_function(|_, ident: String| {
            Ok(HclBlock {
                ident,
                labels: Rc::new(RefCell::new(Vec::new())),
                body: HclBody::default(),
                comment: Rc::new(RefCell::new(None)),
            })
        })?,
    )?;

    // hcl.expr(text) -> HclExpr
    // Parse a raw HCL expression (bare keyword, traversal `var.foo`,
    // function call, ...). Emitted verbatim, unquoted.
    module.set(
        "expr",
        lua.create_function(|_, text: String| -> Result<HclExpr> {
            let expr: hcl::Expression = text
                .parse()
                .map_err(|e| to_err(format!("hcl expression error: {e}")))?;
            Ok(HclExpr(expr))
        })?,
    )?;

    // hcl.ident(name) -> HclExpr
    // A bare identifier/variable reference (e.g. `string`), validated.
    module.set(
        "ident",
        lua.create_function(|_, name: String| -> Result<HclExpr> {
            let var = hcl::Variable::new(name.as_str())
                .map_err(|e| to_err(format!("hcl identifier error: {e}")))?;
            Ok(HclExpr(hcl::Expression::Variable(var)))
        })?,
    )?;

    // hcl.edit(text) -> HclDoc
    // Parse HCL into a format-preserving editable handle. Unlike `parse`
    // (which is lossy and yields plain Lua values), this keeps the
    // document's layout and comments; edits are surgical.
    module.set(
        "edit",
        lua.create_function(|_, text: String| -> Result<HclDoc> {
            let body: hcl::edit::structure::Body = text
                .parse()
                .map_err(|e| to_err(format!("hcl parse error: {e}")))?;
            Ok(HclDoc::new(Rc::new(RefCell::new(body))))
        })?,
    )?;

    lua.globals().set("hcl", module)?;
    Ok(())
}
