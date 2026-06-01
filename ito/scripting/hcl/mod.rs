//! `hcl` module: parse HCL into native Rhai values, and build HCL
//! documents with a fluent builder.
//!
//! Parsing:
//! - `hcl::parse(text)` deserializes via the `hcl-rs` crate into an
//!   `hcl::Value`, then converts to a Rhai `Dynamic` (maps, arrays,
//!   strings, ints/floats, bools, ()), so scripts can inspect HCL
//!   content with ordinary Rhai indexing and iteration.
//!
//! Building:
//! - `hcl::builder()` starts a body (the top-level document).
//! - `hcl::block(ident)` starts a block.
//! - Both expose chainable `.attribute(key, value)` and `.block(child)`;
//!   blocks additionally have `.label(text)`.
//! - `body.to_string()` renders the document as HCL text.
//!
//! Builders carry shared (`Rc<RefCell<..>>`) state, so they behave like
//! handles: chaining and aliasing both observe the same accumulator.
//! Attribute values accept any Rhai value (maps, arrays, scalars) and
//! are converted to HCL expressions.
//!
//! Editing:
//! - `hcl::edit(text)` parses HCL into an editable, format-preserving
//!   handle (`HclDoc`). A cursor/path API navigates and mutates it
//!   in place while keeping layout and comments intact; `.to_string()`
//!   renders the result. See the `edit` submodule.

mod convert;
mod edit;
mod types;
mod visit;

use std::cell::RefCell;
use std::rc::Rc;

use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Module};

use edit::HclDoc;

/// A raw HCL expression value (bare keyword, traversal, function call,
/// etc.), produced by `hcl::expr`/`hcl::ident`. Unlike a plain string
/// attribute value (which is emitted quoted), this is rendered verbatim.
#[derive(Clone)]
pub struct HclExpr(hcl::Expression);

impl HclExpr {
    /// The wrapped value-model expression. Used by the `hcl::edit` cursor
    /// to bridge an `hcl::expr`/`hcl::ident` value into the edit CST.
    pub(super) fn into_inner(self) -> hcl::Expression {
        self.0
    }

    /// Wrap a value-model expression. Used by the `hcl::edit` cursor's
    /// `read_raw` to hand back a computed expression as an `HclExpr`.
    pub(super) fn from_expression(expr: hcl::Expression) -> Self {
        HclExpr(expr)
    }
}

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

fn to_err(msg: impl std::fmt::Display) -> Box<EvalAltResult> {
    msg.to_string().into()
}

/// Convert a Rhai value into an HCL expression. A raw `HclExpr` wrapper
/// (from `hcl::expr`/`hcl::ident`) passes through verbatim; anything
/// else goes through serde and is treated as a literal value.
fn to_expr(value: Dynamic) -> Result<hcl::Expression, Box<EvalAltResult>> {
    if value.is::<HclExpr>() {
        return Ok(value.cast::<HclExpr>().0);
    }
    let json: serde_json::Value = rhai::serde::from_dynamic(&value)?;
    hcl::to_expression(json).map_err(|e| to_err(format!("hcl value error: {e}")))
}

fn ident(name: &str) -> Result<hcl::Identifier, Box<EvalAltResult>> {
    hcl::Identifier::new(name).map_err(|e| to_err(format!("hcl identifier error: {e}")))
}

/// Validate a comment string: non-empty, and every line (after optional
/// leading whitespace) must start with an HCL line-comment marker (`#`
/// or `//`). The caller supplies the markers; we never add them.
fn validate_comment(text: &str) -> Result<(), Box<EvalAltResult>> {
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
    fn add_attribute(&mut self, key: &str, value: Dynamic) -> Result<(), Box<EvalAltResult>> {
        let attr = hcl::Attribute::new(ident(key)?, to_expr(value)?);
        self.items.borrow_mut().push(Item::Attribute(attr, None));
        Ok(())
    }

    fn add_block(&mut self, block: &HclBlock) -> Result<(), Box<EvalAltResult>> {
        self.items.borrow_mut().push(Item::Block(block.clone()));
        Ok(())
    }

    /// Attach a comment per the placement rules: empty body → set the
    /// standalone `header` (case (a)); otherwise comment the last item
    /// (case (c)).
    fn add_comment(&mut self, text: &str) -> Result<(), Box<EvalAltResult>> {
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
    fn render(&self, depth: usize) -> Result<String, Box<EvalAltResult>> {
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
    fn add_comment(&mut self, text: &str) -> Result<(), Box<EvalAltResult>> {
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
    fn render_header(&self, pad: &str) -> Result<String, Box<EvalAltResult>> {
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
    /// into the `hcl::edit` CST at the right nesting level.
    pub(super) fn render(&self, depth: usize) -> Result<String, Box<EvalAltResult>> {
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
    pub(super) fn to_edit_block(
        &self,
        depth: usize,
    ) -> Result<hcl::edit::structure::Block, Box<EvalAltResult>> {
        let text = self.render(depth)?;
        let body: hcl::edit::structure::Body = text
            .parse()
            .map_err(|e| to_err(format!("hcl block build error: {e}")))?;
        body.into_blocks()
            .next()
            .ok_or_else(|| to_err("hcl block build error: no block produced"))
    }
}

/// Register the `hcl` module on `engine`.
pub fn register(engine: &mut Engine) {
    engine.register_type_with_name::<HclBody>("HclBody");
    engine.register_type_with_name::<HclBlock>("HclBlock");
    engine.register_type_with_name::<HclExpr>("HclExpr");
    engine.register_type_with_name::<HclDoc>("HclDoc");

    let mut module = Module::new();

    // hcl::parse(text) -> Dynamic
    module.set_native_fn(
        "parse",
        |text: ImmutableString| -> Result<Dynamic, Box<EvalAltResult>> {
            let value: hcl::Value =
                hcl::from_str(&text).map_err(|e| to_err(format!("hcl parse error: {e}")))?;
            rhai::serde::to_dynamic(value)
        },
    );

    // hcl::to_string(value) -> String
    // Serialize a Rhai value (e.g. a parsed/constructed map) to HCL
    // text. Like toml, this does not necessarily round-trip.
    module.set_native_fn(
        "to_string",
        |value: Dynamic| -> Result<String, Box<EvalAltResult>> {
            let v: hcl::Value = rhai::serde::from_dynamic(&value)?;
            hcl::to_string(&v).map_err(|e| to_err(format!("hcl serialize error: {e}")))
        },
    );

    // hcl::builder() -> HclBody
    module.set_native_fn("builder", || Ok(HclBody::default()));

    // hcl::block(ident) -> HclBlock
    module.set_native_fn("block", |ident: ImmutableString| {
        Ok(HclBlock {
            ident: ident.to_string(),
            labels: Rc::new(RefCell::new(Vec::new())),
            body: HclBody::default(),
            comment: Rc::new(RefCell::new(None)),
        })
    });

    // hcl::expr(text) -> HclExpr
    // Parse a raw HCL expression (bare keyword, traversal `var.foo`,
    // function call, ...). Emitted verbatim, unquoted.
    module.set_native_fn(
        "expr",
        |text: ImmutableString| -> Result<HclExpr, Box<EvalAltResult>> {
            let expr: hcl::Expression = text
                .parse()
                .map_err(|e| to_err(format!("hcl expression error: {e}")))?;
            Ok(HclExpr(expr))
        },
    );

    // hcl::ident(name) -> HclExpr
    // A bare identifier/variable reference (e.g. `string`), validated.
    module.set_native_fn(
        "ident",
        |name: ImmutableString| -> Result<HclExpr, Box<EvalAltResult>> {
            let var = hcl::Variable::new(name.as_str())
                .map_err(|e| to_err(format!("hcl identifier error: {e}")))?;
            Ok(HclExpr(hcl::Expression::Variable(var)))
        },
    );

    // hcl::edit(text) -> HclDoc
    // Parse HCL into a format-preserving editable handle. Unlike `parse`
    // (which is lossy and yields plain Rhai values), this keeps the
    // document's layout and comments; edits are surgical.
    module.set_native_fn(
        "edit",
        |text: ImmutableString| -> Result<HclDoc, Box<EvalAltResult>> {
            let body: hcl::edit::structure::Body = text
                .parse()
                .map_err(|e| to_err(format!("hcl parse error: {e}")))?;
            Ok(HclDoc::new(Rc::new(RefCell::new(body))))
        },
    );

    engine.register_static_module("hcl", module.into());

    register_edit_methods(engine);

    // Body methods (chainable: return the same handle).
    engine.register_fn(
        "attribute",
        |body: &mut HclBody,
         key: ImmutableString,
         value: Dynamic|
         -> Result<HclBody, Box<EvalAltResult>> {
            body.add_attribute(&key, value)?;
            Ok(body.clone())
        },
    );
    engine.register_fn(
        "block",
        |body: &mut HclBody, block: HclBlock| -> Result<HclBody, Box<EvalAltResult>> {
            body.add_block(&block)?;
            Ok(body.clone())
        },
    );
    engine.register_fn(
        "with_comment",
        |body: &mut HclBody, text: ImmutableString| -> Result<HclBody, Box<EvalAltResult>> {
            body.add_comment(&text)?;
            Ok(body.clone())
        },
    );
    engine.register_fn(
        "to_string",
        |body: &mut HclBody| -> Result<String, Box<EvalAltResult>> { body.render(0) },
    );

    // Block methods (chainable: return the same handle).
    engine.register_fn("label", |block: &mut HclBlock, label: ImmutableString| {
        block.labels.borrow_mut().push(label.to_string());
        block.clone()
    });
    engine.register_fn(
        "attribute",
        |block: &mut HclBlock,
         key: ImmutableString,
         value: Dynamic|
         -> Result<HclBlock, Box<EvalAltResult>> {
            block.body.add_attribute(&key, value)?;
            Ok(block.clone())
        },
    );
    engine.register_fn(
        "block",
        |block: &mut HclBlock, child: HclBlock| -> Result<HclBlock, Box<EvalAltResult>> {
            block.body.add_block(&child)?;
            Ok(block.clone())
        },
    );
    engine.register_fn(
        "with_comment",
        |block: &mut HclBlock, text: ImmutableString| -> Result<HclBlock, Box<EvalAltResult>> {
            block.add_comment(&text)?;
            Ok(block.clone())
        },
    );
}

/// Register the cursor methods of the `hcl::edit` handle (`HclDoc`). These
/// dispatch on an `HclDoc` receiver, so the names they share with the
/// builder (`block`, `attribute`, `to_string`) do not collide. Mirrors the
/// `hrs` crate's `script.rs`.
fn register_edit_methods(engine: &mut Engine) {
    // Traversal.
    engine.register_indexer_get(HclDoc::rhai_traverse_str);
    engine.register_indexer_get(HclDoc::rhai_traverse_index);

    engine.register_fn("attr", HclDoc::rhai_attr);
    engine.register_fn("attribute", HclDoc::rhai_attr);

    engine.register_fn("block", |h: &mut HclDoc, ident: &str| {
        h.traverse_block(ident.into(), HclDoc::any_labels(), None)
    });
    engine.register_fn("block", |h: &mut HclDoc, ident: &str, l1: &str| {
        h.traverse_block(ident.into(), vec![l1.into()], None)
    });
    engine.register_fn("block", |h: &mut HclDoc, ident: &str, l1: &str, l2: &str| {
        h.traverse_block(ident.into(), vec![l1.into(), l2.into()], None)
    });
    engine.register_fn(
        "block",
        |h: &mut HclDoc, ident: &str, nth: i64| -> Result<HclDoc, Box<EvalAltResult>> {
            Ok(h.traverse_block(ident.into(), HclDoc::any_labels(), Some(HclDoc::check_nth(nth)?)))
        },
    );
    engine.register_fn(
        "block",
        |h: &mut HclDoc, ident: &str, labels: rhai::Array| -> Result<HclDoc, Box<EvalAltResult>> {
            Ok(h.traverse_block(ident.into(), HclDoc::parse_labels_array(labels)?, None))
        },
    );
    engine.register_fn(
        "block",
        |h: &mut HclDoc,
         ident: &str,
         labels: rhai::Array,
         nth: i64|
         -> Result<HclDoc, Box<EvalAltResult>> {
            Ok(h.traverse_block(
                ident.into(),
                HclDoc::parse_labels_array(labels)?,
                Some(HclDoc::check_nth(nth)?),
            ))
        },
    );
    engine.register_fn(
        "block",
        |h: &mut HclDoc, ident: &str, l1: &str, nth: i64| -> Result<HclDoc, Box<EvalAltResult>> {
            Ok(h.traverse_block(ident.into(), vec![l1.into()], Some(HclDoc::check_nth(nth)?)))
        },
    );
    engine.register_fn(
        "block",
        |h: &mut HclDoc,
         ident: &str,
         l1: &str,
         l2: &str,
         nth: i64|
         -> Result<HclDoc, Box<EvalAltResult>> {
            Ok(h.traverse_block(
                ident.into(),
                vec![l1.into(), l2.into()],
                Some(HclDoc::check_nth(nth)?),
            ))
        },
    );
    engine.register_fn(
        "block",
        |h: &mut HclDoc, ident: &str, l1: &str, l2: &str, l3: &str| {
            h.traverse_block(ident.into(), vec![l1.into(), l2.into(), l3.into()], None)
        },
    );
    engine.register_fn(
        "block",
        |h: &mut HclDoc,
         ident: &str,
         l1: &str,
         l2: &str,
         l3: &str,
         nth: i64|
         -> Result<HclDoc, Box<EvalAltResult>> {
            Ok(h.traverse_block(
                ident.into(),
                vec![l1.into(), l2.into(), l3.into()],
                Some(HclDoc::check_nth(nth)?),
            ))
        },
    );

    engine.register_fn("exists", HclDoc::rhai_exists);

    engine.register_fn("attrs", HclDoc::rhai_attributes);
    engine.register_fn("attributes", HclDoc::rhai_attributes);
    engine.register_fn("try_attrs", HclDoc::rhai_try_attributes);
    engine.register_fn("try_attributes", HclDoc::rhai_try_attributes);

    engine.register_fn("attr_keys", HclDoc::rhai_attribute_keys);
    engine.register_fn("attribute_keys", HclDoc::rhai_attribute_keys);
    engine.register_fn("try_attr_keys", HclDoc::rhai_try_attribute_keys);
    engine.register_fn("try_attribute_keys", HclDoc::rhai_try_attribute_keys);

    engine.register_fn("blocks", HclDoc::rhai_blocks);
    engine.register_fn("blocks", HclDoc::rhai_blocks_filtered);
    engine.register_fn(
        "blocks",
        |h: &mut HclDoc, ident: &str| -> Result<rhai::Array, Box<EvalAltResult>> {
            h.rhai_blocks_filtered(ident, HclDoc::any_labels_array())
        },
    );
    engine.register_fn(
        "blocks",
        |h: &mut HclDoc, ident: &str, l1: &str| -> Result<rhai::Array, Box<EvalAltResult>> {
            h.rhai_blocks_filtered(ident, vec![Dynamic::from(l1.to_string())])
        },
    );
    engine.register_fn(
        "blocks",
        |h: &mut HclDoc,
         ident: &str,
         l1: &str,
         l2: &str|
         -> Result<rhai::Array, Box<EvalAltResult>> {
            h.rhai_blocks_filtered(
                ident,
                vec![Dynamic::from(l1.to_string()), Dynamic::from(l2.to_string())],
            )
        },
    );
    engine.register_fn(
        "blocks",
        |h: &mut HclDoc,
         ident: &str,
         l1: &str,
         l2: &str,
         l3: &str|
         -> Result<rhai::Array, Box<EvalAltResult>> {
            h.rhai_blocks_filtered(
                ident,
                vec![
                    Dynamic::from(l1.to_string()),
                    Dynamic::from(l2.to_string()),
                    Dynamic::from(l3.to_string()),
                ],
            )
        },
    );
    engine.register_fn("try_blocks", HclDoc::rhai_try_blocks);
    engine.register_fn("try_blocks", HclDoc::rhai_try_blocks_filtered);
    engine.register_fn("try_blocks", |h: &mut HclDoc, ident: &str| -> Dynamic {
        h.rhai_try_blocks_filtered(ident, HclDoc::any_labels_array())
    });
    engine.register_fn(
        "try_blocks",
        |h: &mut HclDoc, ident: &str, l1: &str| -> Dynamic {
            h.rhai_try_blocks_filtered(ident, vec![Dynamic::from(l1.to_string())])
        },
    );
    engine.register_fn(
        "try_blocks",
        |h: &mut HclDoc, ident: &str, l1: &str, l2: &str| -> Dynamic {
            h.rhai_try_blocks_filtered(
                ident,
                vec![Dynamic::from(l1.to_string()), Dynamic::from(l2.to_string())],
            )
        },
    );
    engine.register_fn(
        "try_blocks",
        |h: &mut HclDoc, ident: &str, l1: &str, l2: &str, l3: &str| -> Dynamic {
            h.rhai_try_blocks_filtered(
                ident,
                vec![
                    Dynamic::from(l1.to_string()),
                    Dynamic::from(l2.to_string()),
                    Dynamic::from(l3.to_string()),
                ],
            )
        },
    );

    engine.register_fn("block_types", HclDoc::rhai_block_types);
    engine.register_fn("try_block_types", HclDoc::rhai_try_block_types);

    engine.register_fn("block_labels", HclDoc::rhai_block_labels);
    engine.register_fn("try_block_labels", HclDoc::rhai_try_block_labels);

    engine.register_fn("block_count", HclDoc::rhai_block_count);
    engine.register_fn(
        "block_count",
        |h: &mut HclDoc, ident: &str| -> Result<i64, Box<EvalAltResult>> {
            h.rhai_block_count(ident, HclDoc::any_labels_array())
        },
    );
    engine.register_fn(
        "block_count",
        |h: &mut HclDoc, ident: &str, l1: &str| -> Result<i64, Box<EvalAltResult>> {
            h.rhai_block_count(ident, vec![Dynamic::from(l1.to_string())])
        },
    );
    engine.register_fn(
        "block_count",
        |h: &mut HclDoc, ident: &str, l1: &str, l2: &str| -> Result<i64, Box<EvalAltResult>> {
            h.rhai_block_count(
                ident,
                vec![Dynamic::from(l1.to_string()), Dynamic::from(l2.to_string())],
            )
        },
    );
    engine.register_fn(
        "block_count",
        |h: &mut HclDoc,
         ident: &str,
         l1: &str,
         l2: &str,
         l3: &str|
         -> Result<i64, Box<EvalAltResult>> {
            h.rhai_block_count(
                ident,
                vec![
                    Dynamic::from(l1.to_string()),
                    Dynamic::from(l2.to_string()),
                    Dynamic::from(l3.to_string()),
                ],
            )
        },
    );
    engine.register_fn("try_block_count", HclDoc::rhai_try_block_count);
    engine.register_fn("try_block_count", |h: &mut HclDoc, ident: &str| -> Dynamic {
        h.rhai_try_block_count(ident, HclDoc::any_labels_array())
    });
    engine.register_fn(
        "try_block_count",
        |h: &mut HclDoc, ident: &str, l1: &str| -> Dynamic {
            h.rhai_try_block_count(ident, vec![Dynamic::from(l1.to_string())])
        },
    );
    engine.register_fn(
        "try_block_count",
        |h: &mut HclDoc, ident: &str, l1: &str, l2: &str| -> Dynamic {
            h.rhai_try_block_count(
                ident,
                vec![Dynamic::from(l1.to_string()), Dynamic::from(l2.to_string())],
            )
        },
    );
    engine.register_fn(
        "try_block_count",
        |h: &mut HclDoc, ident: &str, l1: &str, l2: &str, l3: &str| -> Dynamic {
            h.rhai_try_block_count(
                ident,
                vec![
                    Dynamic::from(l1.to_string()),
                    Dynamic::from(l2.to_string()),
                    Dynamic::from(l3.to_string()),
                ],
            )
        },
    );

    // Type checks.
    engine.register_fn("is_array", HclDoc::rhai_is_array);
    engine.register_fn("is_object", HclDoc::rhai_is_object);
    engine.register_fn("is_expr", HclDoc::rhai_is_expr);
    engine.register_fn("is_block", HclDoc::rhai_is_block);
    engine.register_fn("is_body", HclDoc::rhai_is_body);

    // Read / modify.
    engine.register_fn("read", HclDoc::rhai_read);
    engine.register_fn("try_read", HclDoc::rhai_try_read);
    engine.register_fn("read_raw", HclDoc::rhai_read_raw);
    engine.register_fn("try_read_raw", HclDoc::rhai_try_read_raw);

    engine.register_fn("write", HclDoc::rhai_write);
    engine.register_fn("try_write", HclDoc::rhai_try_write);

    engine.register_fn("len", HclDoc::rhai_len);
    engine.register_fn("try_len", HclDoc::rhai_try_len);

    engine.register_fn("add", HclDoc::rhai_add_index);
    engine.register_fn("add", HclDoc::rhai_add_kv);
    engine.register_fn("add", HclDoc::rhai_add_value);
    engine.register_fn("try_add", HclDoc::rhai_try_add_index);
    engine.register_fn("try_add", HclDoc::rhai_try_add_kv);
    engine.register_fn("try_add", HclDoc::rhai_try_add_value);

    engine.register_fn("remove", HclDoc::rhai_remove);
    engine.register_fn("remove", HclDoc::rhai_remove_index);
    engine.register_fn("remove", HclDoc::rhai_remove_key);
    engine.register_fn("try_remove", HclDoc::rhai_try_remove);
    engine.register_fn("try_remove", HclDoc::rhai_try_remove_index);
    engine.register_fn("try_remove", HclDoc::rhai_try_remove_key);

    engine.register_fn("to_string", HclDoc::rhai_to_string);
    engine.register_fn("try_to_string", HclDoc::rhai_try_to_string);
}
