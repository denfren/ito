//! `HclDoc`: the editable, format-preserving HCL handle returned by
//! `hcl::edit(text)`. A cursor/path model over a single `hcl::edit`
//! `Body`: traversal calls build up a `path`, and read/write/add/remove
//! resolve that path against the shared body. Ported from the `hrs`
//! crate's `Hcl` type, adapted to a single root body.

use std::cell::RefCell;
use std::rc::Rc;

use hcl::edit::expr::{Expression, ObjectKey};
use hcl::edit::structure::{Attribute, Body, Structure};
use hcl::edit::{Decorate, Ident};
use rhai::{Dynamic, EvalAltResult, Position};

use super::convert::{hcl_to_rhai, rhai_to_hcl};
use super::types::{HclError, Segment, Target, labels_match};
use super::visit;

/// An editable HCL document. Cloning yields a handle that shares the same
/// underlying body (via `Rc<RefCell<..>>`) but carries its own navigation
/// `path`, so traversal is non-destructive and aliasable.
#[derive(Clone)]
pub struct HclDoc {
    root: Rc<RefCell<Body>>,
    path: Vec<Segment>,
}

impl HclDoc {
    pub fn new(root: Rc<RefCell<Body>>) -> Self {
        Self { root, path: vec![] }
    }

    pub fn rhai_traverse_str(&mut self, path: &str) -> Result<Self, Box<EvalAltResult>> {
        Ok(self.traverse(path))
    }

    pub fn rhai_traverse_index(&mut self, index: i64) -> Result<Self, Box<EvalAltResult>> {
        if index < 0 {
            return Err(Box::new(EvalAltResult::ErrorRuntime(
                format!("Index cannot be negative: {}", index).into(),
                Position::NONE,
            )));
        }
        Ok(self.traverse(index as usize))
    }

    pub fn traverse_block(&self, ident: String, labels: Vec<String>, nth: Option<usize>) -> Self {
        let mut new = self.clone();
        new.path.push(Segment::Block { ident, labels, nth });
        new
    }

    /// The label pattern used when a `block`/`blocks`/`block_count` call
    /// names only an identifier (no labels argument): match a block with
    /// **any** labels (`["**"]`). An explicitly passed empty array means
    /// exactly zero labels instead.
    pub fn any_labels() -> Vec<String> {
        vec!["**".to_string()]
    }

    /// The same "any labels" default as a Rhai array, for the
    /// `blocks`/`block_count` entry points that take a `rhai::Array`.
    pub fn any_labels_array() -> rhai::Array {
        vec![Dynamic::from("**".to_string())]
    }

    pub fn check_nth(nth: i64) -> Result<usize, Box<EvalAltResult>> {
        if nth < 0 {
            return Err(Box::new(EvalAltResult::ErrorRuntime(
                format!("Block index cannot be negative: {}", nth).into(),
                Position::NONE,
            )));
        }
        Ok(nth as usize)
    }

    pub fn parse_labels_array(labels: rhai::Array) -> Result<Vec<String>, Box<EvalAltResult>> {
        labels
            .into_iter()
            .map(|v| {
                v.into_string().map_err(|e| {
                    Box::new(EvalAltResult::ErrorRuntime(
                        format!("Labels must be strings: {}", e).into(),
                        Position::NONE,
                    ))
                })
            })
            .collect()
    }

    pub fn rhai_attr(&mut self, key: &str) -> Self {
        let mut new = self.clone();
        new.path.push(Segment::Attr {
            key: key.to_string(),
        });
        new
    }

    pub fn traverse(&self, segment: impl Into<Segment>) -> Self {
        let mut new = self.clone();
        new.path.push(segment.into());
        new
    }

    pub fn to_string(&self) -> Result<String, String> {
        let mut body = self.root.borrow_mut();
        if self.path.is_empty() {
            return Ok(body.to_string());
        }

        let target =
            visit::VisitPath::find(self.path.clone(), &mut body).map_err(|e| e.to_string())?;

        Ok(match target {
            Target::Body(body) => body.to_string(),
            Target::Block(block) => hcl::edit::structure::BodyBuilder::default()
                .block(block.clone())
                .build()
                .to_string(),
            Target::Object(obj) => Expression::from(obj.clone()).to_string(),
            Target::Array(arr) => Expression::from(arr.clone()).to_string(),
            Target::Expr(expr) => expr.to_string(),
        })
    }

    pub fn rhai_to_string(&mut self) -> Result<String, Box<EvalAltResult>> {
        self.to_string()
            .map_err(|e| Box::new(EvalAltResult::ErrorRuntime(e.into(), Position::NONE)))
    }

    pub fn rhai_read(&mut self) -> Result<Dynamic, Box<EvalAltResult>> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Expr(expression) => Ok(hcl_to_rhai(expression)?),
            Target::Object(object) => Ok(hcl_to_rhai(&Expression::Object(object.clone()))?),
            Target::Array(array) => Ok(hcl_to_rhai(&Expression::Array(array.clone()))?),
            _ => Err(HclError::InvalidType {
                expected: "Expr, Object, or Array",
                actual: target.type_name(),
            })?,
        }
    }

    /// Read the expression at the path **verbatim**, as a raw `HclExpr`
    /// (the same value `hcl::expr`/`hcl::ident` produce). Unlike `read`,
    /// this works for computed expressions — `var.x`, function calls,
    /// `string`, `list(string)`, conditionals — and round-trips straight
    /// back into `write`/`add`. The handle must point at an expression
    /// (attribute value / array element), not a body or block.
    pub fn rhai_read_raw(&mut self) -> Result<super::HclExpr, Box<EvalAltResult>> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        let cst: Expression = match target {
            Target::Expr(expression) => expression.clone(),
            Target::Object(object) => Expression::Object(object.clone()),
            Target::Array(array) => Expression::Array(array.clone()),
            _ => Err(HclError::InvalidType {
                expected: "Expr, Object, or Array",
                actual: target.type_name(),
            })?,
        };
        // Bridge the edit CST expression back to the value-model
        // expression that `HclExpr` wraps (lossless, decor dropped).
        Ok(super::HclExpr::from_expression(hcl::Expression::from(cst)))
    }

    pub fn rhai_try_read_raw(&mut self) -> Dynamic {
        self.rhai_read_raw()
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT)
    }

    pub fn rhai_write(&mut self, value: Dynamic) -> Result<(), Box<EvalAltResult>> {
        // A builder block replaces the block the cursor points at, keeping
        // the original's position and indentation.
        if value.is::<super::HclBlock>() {
            let builder = value.cast::<super::HclBlock>();
            let mut root = self.root.borrow_mut();
            let path_depth = self.path.len();
            let (target, segment) = visit::VisitPath::find_parent(self.path.clone(), &mut root)?;
            let Segment::Block { ident, labels, nth } = &segment else {
                return Err(HclError::InvalidType {
                    expected: "Block segment",
                    actual: segment.kind_name(),
                })?;
            };
            let body: &mut Body = match target {
                Target::Body(body) => body,
                Target::Block(parent) => &mut parent.body,
                _ => Err(HclError::InvalidType {
                    expected: "Body or Block",
                    actual: target.type_name(),
                })?,
            };
            let depth = path_depth.saturating_sub(1);
            let idx = Self::find_block_index(body, ident, labels, *nth).ok_or_else(|| {
                HclError::NotFound {
                    path: self.path.clone(),
                    segment: segment.clone(),
                }
            })?;
            // Keep the replaced block's own indentation.
            let indent = Self::structure_indent(body.iter().nth(idx).cloned())
                .unwrap_or_else(|| "  ".repeat(depth));
            let block = Self::block_at_indent(&builder, &indent)?;
            body.remove(idx);
            body.insert(idx, block);
            return Ok(());
        }

        let mut root = self.root.borrow_mut();
        let (target, segment) = visit::VisitPath::find_parent(self.path.clone(), &mut root)?;

        let value = rhai_to_hcl(value)?;

        match target {
            Target::Body(body) => {
                let key = segment.as_str().ok_or(HclError::InvalidType {
                    expected: "Text segment",
                    actual: "Index segment",
                })?;
                if let Some(mut attr) = body.get_attribute_mut(key) {
                    *(attr.value_mut()) = value;
                    Ok(())
                } else {
                    Err(HclError::NotFound {
                        path: self.path.clone(),
                        segment,
                    })?
                }
            }
            Target::Block(block) => {
                let key = segment.as_str().ok_or(HclError::InvalidType {
                    expected: "Text segment",
                    actual: "Index segment",
                })?;
                if let Some(mut attr) = block.body.get_attribute_mut(key) {
                    *(attr.value_mut()) = value;
                    Ok(())
                } else {
                    Err(HclError::NotFound {
                        path: self.path.clone(),
                        segment,
                    })?
                }
            }
            Target::Object(object) => {
                let key = segment.as_str().ok_or(HclError::InvalidType {
                    expected: "Text segment",
                    actual: "Index segment",
                })?;
                if let Some(object_value) =
                    object.get_mut(&ObjectKey::Ident(Ident::new_sanitized(key).into()))
                {
                    *object_value.expr_mut() = value;
                    Ok(())
                } else {
                    Err(HclError::NotFound {
                        path: self.path.clone(),
                        segment,
                    })?
                }
            }
            Target::Array(array) => {
                let index = segment.as_index().ok_or(HclError::InvalidType {
                    expected: "Index segment",
                    actual: "Text segment",
                })?;
                if let Some(expr) = array.get_mut(index) {
                    *expr = value;
                    Ok(())
                } else {
                    Err(HclError::IndexOutOfBounds {
                        index: index as i64,
                        len: array.len(),
                    })?
                }
            }
            _ => Err(HclError::InvalidType {
                expected: "Body, Block, Object, or Array",
                actual: target.type_name(),
            })?,
        }
    }

    pub fn rhai_exists(&mut self) -> bool {
        if self.path.is_empty() {
            return true;
        }
        let mut root = self.root.borrow_mut();
        visit::VisitPath::find(self.path.clone(), &mut root).is_ok()
    }

    fn attr_keys_from_body(body: &Body) -> rhai::Array {
        body.attributes()
            .map(|attr| Dynamic::from(attr.key.as_str().to_string()))
            .collect()
    }

    pub fn rhai_attribute_keys(&mut self) -> Result<rhai::Array, Box<EvalAltResult>> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Body(body) => Ok(Self::attr_keys_from_body(body)),
            Target::Block(block) => Ok(Self::attr_keys_from_body(&block.body)),
            _ => Err(HclError::InvalidType {
                expected: "Body or Block",
                actual: target.type_name(),
            })?,
        }
    }

    pub fn rhai_len(&mut self) -> Result<i64, Box<EvalAltResult>> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Body(body) => Ok((body.len()) as i64),
            Target::Block(block) => Ok((block.body.len()) as i64),
            Target::Object(object) => Ok(object.len() as i64),
            Target::Array(array) => Ok(array.len() as i64),
            _ => Err(HclError::InvalidType {
                expected: "Body, Block, Object, or Array",
                actual: target.type_name(),
            })?,
        }
    }

    /// The absolute structure index of the block matching `ident` + `labels`
    /// (label-glob aware) at occurrence `nth` (default 0), if any.
    fn find_block_index(
        body: &Body,
        ident: &str,
        labels: &[String],
        nth: Option<usize>,
    ) -> Option<usize> {
        let target_nth = nth.unwrap_or(0);
        let mut seen = 0usize;
        for (idx, structure) in body.iter().enumerate() {
            if let Some(block) = structure.as_block() {
                let block_labels: Vec<&str> = block.labels.iter().map(|l| l.as_str()).collect();
                if block.ident.value().as_str() == ident && labels_match(&block_labels, labels) {
                    if seen == target_nth {
                        return Some(idx);
                    }
                    seen += 1;
                }
            }
        }
        None
    }

    /// Remove the single block matching `ident` + `labels` (label-glob aware)
    /// at index `nth` (default 0) from `body`. Unlike `Body::remove_blocks`,
    /// which is ident-only and removes *all* matching blocks, this targets the
    /// exact label-addressed block by computing its absolute structure index.
    /// Returns `false` if no such block exists.
    fn remove_block_from_body(
        body: &mut Body,
        ident: &str,
        labels: &[String],
        nth: Option<usize>,
    ) -> bool {
        match Self::find_block_index(body, ident, labels, nth) {
            Some(idx) => {
                body.remove(idx);
                true
            }
            None => false,
        }
    }

    pub fn rhai_remove(&mut self) -> Result<(), Box<EvalAltResult>> {
        let mut root = self.root.borrow_mut();
        let (target, segment) = visit::VisitPath::find_parent(self.path.clone(), &mut root)?;

        match target {
            Target::Body(body) => {
                if let Segment::Block { ident, labels, nth } = &segment {
                    return if Self::remove_block_from_body(body, ident, labels, *nth) {
                        Ok(())
                    } else {
                        Err(HclError::NotFound {
                            path: self.path.clone(),
                            segment,
                        })?
                    };
                }
                let key = segment.as_str().ok_or(HclError::InvalidType {
                    expected: "Text or Block segment",
                    actual: segment.kind_name(),
                })?;
                if body.has_attribute(key) {
                    body.remove_attribute(key);
                    Ok(())
                } else if body.has_blocks(key) {
                    body.remove_blocks(key);
                    Ok(())
                } else {
                    Err(HclError::NotFound {
                        path: self.path.clone(),
                        segment,
                    })?
                }
            }
            Target::Block(block) => {
                if let Segment::Block { ident, labels, nth } = &segment {
                    return if Self::remove_block_from_body(&mut block.body, ident, labels, *nth) {
                        Ok(())
                    } else {
                        Err(HclError::NotFound {
                            path: self.path.clone(),
                            segment,
                        })?
                    };
                }
                let key = segment.as_str().ok_or(HclError::InvalidType {
                    expected: "Text or Block segment",
                    actual: segment.kind_name(),
                })?;
                if block.body.has_attribute(key) {
                    block.body.remove_attribute(key);
                    Ok(())
                } else if block.body.has_blocks(key) {
                    block.body.remove_blocks(key);
                    Ok(())
                } else {
                    Err(HclError::NotFound {
                        path: self.path.clone(),
                        segment,
                    })?
                }
            }
            Target::Object(object) => {
                let key = segment.as_str().ok_or(HclError::InvalidType {
                    expected: "Text segment",
                    actual: "Index segment",
                })?;
                let obj_key = ObjectKey::Ident(Ident::new_sanitized(key).into());
                if object.contains_key(&obj_key) {
                    object.remove(&obj_key);
                    Ok(())
                } else {
                    Err(HclError::NotFound {
                        path: self.path.clone(),
                        segment,
                    })?
                }
            }
            Target::Array(array) => {
                let index = segment.as_index().ok_or(HclError::InvalidType {
                    expected: "Index segment",
                    actual: "Text segment",
                })?;
                if index < array.len() {
                    array.remove(index);
                    Ok(())
                } else {
                    Err(HclError::IndexOutOfBounds {
                        index: index as i64,
                        len: array.len(),
                    })?
                }
            }
            _ => Err(HclError::InvalidType {
                expected: "Body, Block, Object, or Array",
                actual: target.type_name(),
            })?,
        }
    }

    /// The indentation a structure carries: the run of whitespace after the
    /// last newline of its decor prefix (empty if it has none).
    fn structure_indent(structure: Option<hcl::edit::structure::Structure>) -> Option<String> {
        structure
            .as_ref()
            .and_then(|s| s.decor().prefix())
            .map(|p| p.to_string().rsplit('\n').next().unwrap_or("").to_string())
            .filter(|s| !s.is_empty())
    }

    /// Indentation to use for a structure appended to `body`. Prefer the
    /// indent of an existing sibling, so added items line up with what's
    /// already there. For an empty body, fall back to two spaces per nesting
    /// level (`depth`).
    fn body_indent(body: &Body, depth: usize) -> String {
        Self::structure_indent(body.iter().last().cloned()).unwrap_or_else(|| "  ".repeat(depth))
    }

    /// Build an `hcl::block(…)` builder into an edit-CST block laid out at
    /// `indent` (its inner body rendered one step deeper, its own header
    /// prefixed with `indent`). Shared by `add`/`write` so both indent a
    /// spliced-in block identically.
    fn block_at_indent(
        builder: &super::HclBlock,
        indent: &str,
    ) -> Result<hcl::edit::structure::Block, Box<EvalAltResult>> {
        let mut block = builder.to_edit_block(indent.len() / 2)?;
        block.decor_mut().set_prefix(indent.to_string());
        Ok(block)
    }

    /// Set a structure's leading-whitespace prefix to `indent`.
    fn indented(item: impl Into<Structure>, indent: &str) -> Structure {
        let mut s = item.into();
        s.decor_mut().set_prefix(indent.to_string());
        s
    }

    pub fn rhai_add_kv(&mut self, key: &str, value: Dynamic) -> Result<(), Box<EvalAltResult>> {
        let mut root = self.root.borrow_mut();
        let depth = self.path.len();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;
        let value = rhai_to_hcl(value)?;

        match target {
            Target::Body(body) => {
                let indent = Self::body_indent(body, depth);
                body.push(Self::indented(
                    Attribute::new(Ident::new_sanitized(key), value),
                    &indent,
                ));
                Ok(())
            }
            Target::Block(block) => {
                let indent = Self::body_indent(&block.body, depth + 1);
                block.body.push(Self::indented(
                    Attribute::new(Ident::new_sanitized(key), value),
                    &indent,
                ));
                Ok(())
            }
            Target::Object(object) => {
                let obj_key = ObjectKey::Ident(Ident::new_sanitized(key).into());
                object.insert(obj_key, value);
                Ok(())
            }
            _ => Err(HclError::InvalidType {
                expected: "Body, Block, or Object",
                actual: target.type_name(),
            })?,
        }
    }

    pub fn rhai_add_index(&mut self, index: i64, value: Dynamic) -> Result<(), Box<EvalAltResult>> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;
        let value = rhai_to_hcl(value)?;

        match target {
            Target::Array(array) => {
                let idx = if index < 0 {
                    0usize
                } else {
                    (index as usize).min(array.len())
                };
                array.insert(idx, value);
                Ok(())
            }
            _ => Err(HclError::InvalidType {
                expected: "Array",
                actual: target.type_name(),
            })?,
        }
    }

    pub fn rhai_add_value(&mut self, value: Dynamic) -> Result<(), Box<EvalAltResult>> {
        // A builder block (`hcl::block(..)`) appends a nested block to a
        // body/block target, indented to match its siblings.
        if value.is::<super::HclBlock>() {
            let builder = value.cast::<super::HclBlock>();
            let mut root = self.root.borrow_mut();
            let path_depth = self.path.len();
            let target = visit::VisitPath::find(self.path.clone(), &mut root)?;
            let (body, depth): (&mut Body, usize) = match target {
                Target::Body(body) => (body, path_depth),
                Target::Block(parent) => (&mut parent.body, path_depth + 1),
                _ => Err(HclError::InvalidType {
                    expected: "Body or Block",
                    actual: target.type_name(),
                })?,
            };
            // Render at the body's actual indent depth (inferred from an
            // existing sibling when present, else from the path depth), so
            // the block and its nested content line up.
            let indent = Self::body_indent(body, depth);
            body.push(Self::block_at_indent(&builder, &indent)?);
            return Ok(());
        }

        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;
        let value = rhai_to_hcl(value)?;

        match target {
            Target::Array(array) => {
                array.push(value);
                Ok(())
            }
            _ => Err(HclError::InvalidType {
                expected: "Array",
                actual: target.type_name(),
            })?,
        }
    }

    pub fn rhai_remove_key(&mut self, key: &str) -> Result<(), Box<EvalAltResult>> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Body(body) => {
                if body.has_attribute(key) {
                    body.remove_attribute(key);
                    Ok(())
                } else if body.has_blocks(key) {
                    body.remove_blocks(key);
                    Ok(())
                } else {
                    Err(HclError::NotFound {
                        path: self.path.clone(),
                        segment: key.into(),
                    })?
                }
            }
            Target::Block(block) => {
                if block.body.has_attribute(key) {
                    block.body.remove_attribute(key);
                    Ok(())
                } else if block.body.has_blocks(key) {
                    block.body.remove_blocks(key);
                    Ok(())
                } else {
                    Err(HclError::NotFound {
                        path: self.path.clone(),
                        segment: key.into(),
                    })?
                }
            }
            Target::Object(object) => {
                let obj_key = ObjectKey::Ident(Ident::new_sanitized(key).into());
                if object.contains_key(&obj_key) {
                    object.remove(&obj_key);
                    Ok(())
                } else {
                    Err(HclError::NotFound {
                        path: self.path.clone(),
                        segment: key.into(),
                    })?
                }
            }
            _ => Err(HclError::InvalidType {
                expected: "Body, Block, or Object",
                actual: target.type_name(),
            })?,
        }
    }

    pub fn rhai_remove_index(&mut self, index: i64) -> Result<(), Box<EvalAltResult>> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Array(array) => {
                if index >= 0 && (index as usize) < array.len() {
                    array.remove(index as usize);
                    Ok(())
                } else {
                    Err(HclError::IndexOutOfBounds {
                        index,
                        len: array.len(),
                    })?
                }
            }
            _ => Err(HclError::InvalidType {
                expected: "Array",
                actual: target.type_name(),
            })?,
        }
    }

    fn block_types_from_body(body: &Body) -> rhai::Array {
        let mut seen = Vec::new();
        for block in body.blocks() {
            let ident = block.ident.as_str().to_string();
            if !seen.contains(&ident) {
                seen.push(ident);
            }
        }
        seen.into_iter().map(Dynamic::from).collect()
    }

    pub fn rhai_block_types(&mut self) -> Result<rhai::Array, Box<EvalAltResult>> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Body(body) => Ok(Self::block_types_from_body(body)),
            Target::Block(block) => Ok(Self::block_types_from_body(&block.body)),
            _ => Err(HclError::InvalidType {
                expected: "Body or Block",
                actual: target.type_name(),
            })?,
        }
    }

    fn block_labels_from_body(body: &Body, ident: &str) -> rhai::Array {
        let mut seen = Vec::<Vec<String>>::new();
        for block in body.blocks() {
            if block.ident.as_str() != ident {
                continue;
            }
            let labels: Vec<String> = block
                .labels
                .iter()
                .map(|l| l.as_str().to_string())
                .collect();
            if !seen.contains(&labels) {
                seen.push(labels);
            }
        }
        seen.into_iter()
            .map(|labels| Dynamic::from_array(labels.into_iter().map(Dynamic::from).collect()))
            .collect()
    }

    pub fn rhai_block_labels(&mut self, ident: &str) -> Result<rhai::Array, Box<EvalAltResult>> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Body(body) => Ok(Self::block_labels_from_body(body, ident)),
            Target::Block(block) => Ok(Self::block_labels_from_body(&block.body, ident)),
            _ => Err(HclError::InvalidType {
                expected: "Body or Block",
                actual: target.type_name(),
            })?,
        }
    }

    fn blocks_from_body(
        &self,
        body: &Body,
        filter_ident: Option<&str>,
        filter_labels: &[String],
    ) -> rhai::Array {
        let mut counters: std::collections::HashMap<(String, Vec<String>), usize> =
            std::collections::HashMap::new();
        body.blocks()
            .filter(|block| {
                if let Some(ident) = filter_ident {
                    if block.ident.as_str() != ident {
                        return false;
                    }
                }
                let block_labels: Vec<&str> = block.labels.iter().map(|l| l.as_str()).collect();
                labels_match(&block_labels, filter_labels)
            })
            .map(|block| {
                let ident = block.ident.as_str().to_string();
                let labels: Vec<String> = block
                    .labels
                    .iter()
                    .map(|l| l.as_str().to_string())
                    .collect();
                let key = (ident.clone(), labels.clone());
                let nth = counters.entry(key).or_insert(0);
                let hcl = self.traverse_block(ident, labels, Some(*nth));
                *nth += 1;
                Dynamic::from(hcl)
            })
            .collect()
    }

    fn resolve_and_collect_blocks(
        &mut self,
        filter_ident: Option<&str>,
        filter_labels: &[String],
    ) -> Result<rhai::Array, Box<EvalAltResult>> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Body(body) => Ok(self.blocks_from_body(body, filter_ident, filter_labels)),
            Target::Block(block) => {
                Ok(self.blocks_from_body(&block.body, filter_ident, filter_labels))
            }
            _ => Err(HclError::InvalidType {
                expected: "Body or Block",
                actual: target.type_name(),
            })?,
        }
    }

    pub fn rhai_blocks(&mut self) -> Result<rhai::Array, Box<EvalAltResult>> {
        self.resolve_and_collect_blocks(None, &[])
    }

    pub fn rhai_blocks_filtered(
        &mut self,
        ident: &str,
        labels: rhai::Array,
    ) -> Result<rhai::Array, Box<EvalAltResult>> {
        let labels = Self::parse_labels_array(labels)?;
        self.resolve_and_collect_blocks(Some(ident), &labels)
    }

    fn attributes_from_body(&self, body: &Body) -> rhai::Array {
        body.attributes()
            .map(|attr| {
                let mut new = self.clone();
                new.path.push(Segment::Attr {
                    key: attr.key.as_str().to_string(),
                });
                Dynamic::from(new)
            })
            .collect()
    }

    pub fn rhai_attributes(&mut self) -> Result<rhai::Array, Box<EvalAltResult>> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Body(body) => Ok(self.attributes_from_body(body)),
            Target::Block(block) => Ok(self.attributes_from_body(&block.body)),
            _ => Err(HclError::InvalidType {
                expected: "Body or Block",
                actual: target.type_name(),
            })?,
        }
    }

    fn block_count_from_body(body: &Body, ident: &str, labels: &[String]) -> i64 {
        body.blocks()
            .filter(|b| {
                let block_labels: Vec<&str> = b.labels.iter().map(|l| l.as_str()).collect();
                b.ident.as_str() == ident && labels_match(&block_labels, labels)
            })
            .count() as i64
    }

    pub fn rhai_block_count(
        &mut self,
        ident: &str,
        labels: rhai::Array,
    ) -> Result<i64, Box<EvalAltResult>> {
        let labels = Self::parse_labels_array(labels)?;
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Body(body) => Ok(Self::block_count_from_body(body, ident, &labels)),
            Target::Block(block) => Ok(Self::block_count_from_body(&block.body, ident, &labels)),
            _ => Err(HclError::InvalidType {
                expected: "Body or Block",
                actual: target.type_name(),
            })?,
        }
    }

    pub fn rhai_is_array(&mut self) -> bool {
        if self.path.is_empty() {
            return false;
        }
        let Ok(mut root) = self.root.try_borrow_mut() else {
            return false;
        };
        matches!(
            visit::VisitPath::find(self.path.clone(), &mut root),
            Ok(Target::Array(_))
        )
    }

    pub fn rhai_is_object(&mut self) -> bool {
        if self.path.is_empty() {
            return false;
        }
        let Ok(mut root) = self.root.try_borrow_mut() else {
            return false;
        };
        matches!(
            visit::VisitPath::find(self.path.clone(), &mut root),
            Ok(Target::Object(_))
        )
    }

    pub fn rhai_is_expr(&mut self) -> bool {
        if self.path.is_empty() {
            return false;
        }
        let Ok(mut root) = self.root.try_borrow_mut() else {
            return false;
        };
        matches!(
            visit::VisitPath::find(self.path.clone(), &mut root),
            Ok(Target::Expr(_) | Target::Object(_) | Target::Array(_))
        )
    }

    pub fn rhai_is_block(&mut self) -> bool {
        if self.path.is_empty() {
            return false;
        }
        let Ok(mut root) = self.root.try_borrow_mut() else {
            return false;
        };
        matches!(
            visit::VisitPath::find(self.path.clone(), &mut root),
            Ok(Target::Block(_))
        )
    }

    pub fn rhai_is_body(&mut self) -> bool {
        if self.path.is_empty() {
            return true;
        }
        let Ok(mut root) = self.root.try_borrow_mut() else {
            return false;
        };
        matches!(
            visit::VisitPath::find(self.path.clone(), &mut root),
            Ok(Target::Body(_) | Target::Block(_))
        )
    }

    // Read-like try_ variants: return value on success, () on failure
    pub fn rhai_try_read(&mut self) -> Dynamic {
        self.rhai_read().unwrap_or(Dynamic::UNIT)
    }

    pub fn rhai_try_attribute_keys(&mut self) -> Dynamic {
        self.rhai_attribute_keys()
            .map(Dynamic::from_array)
            .unwrap_or(Dynamic::UNIT)
    }

    pub fn rhai_try_len(&mut self) -> Dynamic {
        self.rhai_len()
            .map(Dynamic::from_int)
            .unwrap_or(Dynamic::UNIT)
    }

    pub fn rhai_try_to_string(&mut self) -> Dynamic {
        self.rhai_to_string()
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT)
    }

    // Write-like try_ variants: return bool
    pub fn rhai_try_write(&mut self, value: Dynamic) -> bool {
        self.rhai_write(value).is_ok()
    }

    pub fn rhai_try_remove(&mut self) -> bool {
        self.rhai_remove().is_ok()
    }

    pub fn rhai_try_remove_key(&mut self, key: &str) -> bool {
        self.rhai_remove_key(key).is_ok()
    }

    pub fn rhai_try_remove_index(&mut self, index: i64) -> bool {
        self.rhai_remove_index(index).is_ok()
    }

    pub fn rhai_try_add_kv(&mut self, key: &str, value: Dynamic) -> bool {
        self.rhai_add_kv(key, value).is_ok()
    }

    pub fn rhai_try_add_index(&mut self, index: i64, value: Dynamic) -> bool {
        self.rhai_add_index(index, value).is_ok()
    }

    pub fn rhai_try_add_value(&mut self, value: Dynamic) -> bool {
        self.rhai_add_value(value).is_ok()
    }

    pub fn rhai_try_blocks(&mut self) -> Dynamic {
        self.rhai_blocks()
            .map(Dynamic::from_array)
            .unwrap_or(Dynamic::UNIT)
    }

    pub fn rhai_try_blocks_filtered(&mut self, ident: &str, labels: rhai::Array) -> Dynamic {
        self.rhai_blocks_filtered(ident, labels)
            .map(Dynamic::from_array)
            .unwrap_or(Dynamic::UNIT)
    }

    pub fn rhai_try_attributes(&mut self) -> Dynamic {
        self.rhai_attributes()
            .map(Dynamic::from_array)
            .unwrap_or(Dynamic::UNIT)
    }

    pub fn rhai_try_block_types(&mut self) -> Dynamic {
        self.rhai_block_types()
            .map(Dynamic::from_array)
            .unwrap_or(Dynamic::UNIT)
    }

    pub fn rhai_try_block_labels(&mut self, ident: &str) -> Dynamic {
        self.rhai_block_labels(ident)
            .map(Dynamic::from_array)
            .unwrap_or(Dynamic::UNIT)
    }

    pub fn rhai_try_block_count(&mut self, ident: &str, labels: rhai::Array) -> Dynamic {
        self.rhai_block_count(ident, labels)
            .map(Dynamic::from_int)
            .unwrap_or(Dynamic::UNIT)
    }
}
