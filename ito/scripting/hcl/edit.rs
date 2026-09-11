//! `HclDoc`: the editable, format-preserving HCL handle returned by
//! `hcl.edit(text)`. A cursor/path model over a single `hcl::edit`
//! `Body`: traversal calls build up a `path`, and read/write/add/remove
//! resolve that path against the shared body. Ported from the `hrs`
//! crate's `Hcl` type, adapted to a single root body.

use std::cell::RefCell;
use std::rc::Rc;

use hcl::edit::expr::Expression;
use hcl::edit::structure::{Attribute, Body, Structure};
use hcl::edit::{Decorate, Ident};
use mlua::{Lua, MetaMethod, UserData, UserDataMethods, Value, Variadic};

use super::convert::{hcl_to_lua, lua_to_hcl};
use super::types::{
    CallbackFailed, HclError, Segment, Target, find_object_key, labels_match, new_object_key,
};
use super::visit;

type Result<T> = mlua::Result<T>;

/// A `try_*` result: a path/structure failure is `false`, but an error the
/// script's own callback raised is not ours to swallow.
fn attempt(r: Result<()>) -> Result<bool> {
    match r {
        Ok(()) => Ok(true),
        Err(e) if e.downcast_ref::<CallbackFailed>().is_some() => Err(e),
        Err(_) => Ok(false),
    }
}

/// The string keys of an object (identifier or quoted-string keys).
fn object_keys(object: &hcl::edit::expr::Object) -> Vec<String> {
    use hcl::edit::expr::ObjectKey;
    object
        .iter()
        .filter_map(|(k, _)| match k {
            ObjectKey::Ident(i) => Some(i.as_str().to_string()),
            ObjectKey::Expression(Expression::String(s)) => Some(s.value().to_string()),
            _ => None,
        })
        .collect()
}

/// Remove the member `key` of a body: the attribute, or the unlabelled block
/// of that name. Both existing is ambiguous; a labelled block is never
/// removed this way (address it with `block(ident, labels)`).
fn remove_named(body: &mut Body, key: &str, path: Vec<Segment>) -> Result<()> {
    let has_attr = body.has_attribute(key);
    let block_idx = HclDoc::find_block_index(body, key, &[], None);
    match (has_attr, block_idx) {
        (true, Some(_)) => Err(HclError::AmbiguousKey { key: key.to_string() })?,
        (true, None) => {
            body.remove_attribute(key);
            Ok(())
        }
        (false, Some(i)) => {
            body.remove(i);
            Ok(())
        }
        (false, None) => Err(HclError::NotFound { path, segment: key.into() })?,
    }
}

/// Convert a script-facing 1-based index to the 0-based position the
/// underlying `hcl::edit` array/path primitives expect (matching Lua's
/// own `string.sub`/`table.insert` convention — index `1` is the first
/// element). Rejects `0` and negative indices.
fn zero_based(index: i64) -> Result<usize> {
    if index < 1 {
        return Err(mlua::Error::runtime(format!(
            "index must be >= 1, got {index}"
        )));
    }
    Ok((index - 1) as usize)
}

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

    fn traverse_str(&self, path: &str) -> Self {
        self.traverse(path)
    }

    /// `index` is 1-based, matching Lua's own indexing convention.
    fn traverse_index(&self, index: i64) -> Result<Self> {
        Ok(self.traverse(zero_based(index)?))
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

    /// `nth` is 1-based, matching Lua's own indexing convention.
    fn check_nth(nth: i64) -> Result<usize> {
        zero_based(nth)
    }

    fn attr(&self, key: &str) -> Self {
        let mut new = self.clone();
        new.path.push(Segment::Attr {
            key: key.to_string(),
        });
        new
    }

    /// The path above the last segment (for `NotFound` errors).
    fn parent_path(&self) -> Vec<Segment> {
        self.path[..self.path.len().saturating_sub(1)].to_vec()
    }

    pub fn traverse(&self, segment: impl Into<Segment>) -> Self {
        let mut new = self.clone();
        new.path.push(segment.into());
        new
    }

    // ---- flexible-argument parsing for the `block*` overload sets ------
    //
    // mlua has no overloading, so a single variadic tail is parsed
    // positionally here. The accepted shapes are:
    //   - (no args)                         -> any labels, no nth
    //   - (label...)                        -> 1..=3 string labels, no nth
    //   - (nth)                             -> any labels, at nth
    //   - (label..., nth)                   -> string labels + trailing nth
    //   - ([labels])                        -> array of labels, no nth
    //   - ([labels], nth)                   -> array of labels + nth

    /// Parse a variadic tail into `(labels, nth)`. `default_any` controls
    /// what an *absent* labels argument means: `["**"]` (match any) for the
    /// `block(...)` navigator, or `["**"]` likewise for filters — callers
    /// that need the "empty = zero labels" semantics pass an explicit array.
    fn parse_labels_and_nth(args: &[Value]) -> Result<(Vec<String>, Option<usize>)> {
        // Split off an optional trailing integer as `nth`.
        let (labels_part, nth) = match args.last() {
            Some(Value::Integer(n)) => (&args[..args.len() - 1], Some(Self::check_nth(*n)?)),
            _ => (args, None),
        };

        // A single table argument is the `[labels]` array form.
        if labels_part.len() == 1
            && let Value::Table(t) = &labels_part[0]
        {
            return Ok((Self::table_labels(t)?, nth));
        }

        // Otherwise every element must be a string label.
        let mut labels = Vec::with_capacity(labels_part.len());
        for v in labels_part {
            match v {
                Value::String(s) => labels.push(s.to_str()?.to_string()),
                _ => {
                    return Err(mlua::Error::runtime(
                        "Labels must be strings (or a single array of strings)",
                    ));
                }
            }
        }
        // No labels given -> match any labels.
        if labels.is_empty() && nth.is_none() && args.is_empty() {
            return Ok((Self::any_labels(), None));
        }
        if labels.is_empty() {
            // `(nth)` alone, or `([])`-less empty tail with an nth: match any.
            return Ok((Self::any_labels(), nth));
        }
        Ok((labels, nth))
    }

    fn table_labels(t: &mlua::Table) -> Result<Vec<String>> {
        t.clone()
            .sequence_values::<String>()
            .collect::<Result<Vec<_>>>()
            .map_err(|_| mlua::Error::runtime("Labels must be strings"))
    }

    /// Parse the labels-only variadic tail for `blocks`/`block_count`: the
    /// same shapes minus a trailing `nth`. An absent labels argument means
    /// "match any" (`["**"]`); an explicit empty array means zero labels.
    fn parse_filter_labels(args: &[Value]) -> Result<Vec<String>> {
        if args.is_empty() {
            return Ok(Self::any_labels());
        }
        if args.len() == 1
            && let Value::Table(t) = &args[0]
        {
            return Ok(Self::table_labels(t)?);
        }
        let mut labels = Vec::with_capacity(args.len());
        for v in args {
            match v {
                Value::String(s) => labels.push(s.to_str()?.to_string()),
                _ => {
                    return Err(mlua::Error::runtime(
                        "Labels must be strings (or a single array of strings)",
                    ));
                }
            }
        }
        Ok(labels)
    }

    // ---- to_string ----------------------------------------------------

    pub fn render_to_string(&self) -> std::result::Result<String, String> {
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

    fn to_string(&self) -> Result<String> {
        self.render_to_string().map_err(mlua::Error::runtime)
    }

    // ---- read ---------------------------------------------------------

    fn read(&self, lua: &Lua) -> Result<Value> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Expr(expression) => Ok(hcl_to_lua(lua, expression)?),
            Target::Object(object) => Ok(hcl_to_lua(lua, &Expression::Object(object.clone()))?),
            Target::Array(array) => Ok(hcl_to_lua(lua, &Expression::Array(array.clone()))?),
            _ => Err(HclError::InvalidType {
                expected: "an expression, object or array",
                actual: target.type_name(),
            })?,
        }
    }

    /// Read the expression at the path **verbatim**, as a raw `HclExpr`
    /// (the same value `hcl.expr`/`hcl.ident` produce). Unlike `read`,
    /// this works for computed expressions — `var.x`, function calls,
    /// `string`, `list(string)`, conditionals — and round-trips straight
    /// back into `write`/`add`. The handle must point at an expression
    /// (attribute value / array element), not a body or block.
    fn raw(&self) -> Result<super::HclExpr> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        let cst: Expression = match target {
            Target::Expr(expression) => expression.clone(),
            Target::Object(object) => Expression::Object(object.clone()),
            Target::Array(array) => Expression::Array(array.clone()),
            _ => Err(HclError::InvalidType {
                expected: "an expression, object or array",
                actual: target.type_name(),
            })?,
        };
        // Bridge the edit CST expression back to the value-model
        // expression that `HclExpr` wraps (lossless, decor dropped).
        Ok(super::HclExpr::from_expression(hcl::Expression::from(cst)))
    }

    // ---- write --------------------------------------------------------

    fn write(&self, lua: &Lua, value: Value) -> Result<()> {
        // A builder block replaces the block the cursor points at, keeping
        // the original's position and indentation.
        if let Value::UserData(ud) = &value
            && ud.is::<super::HclBlock>()
        {
            let builder = ud.borrow::<super::HclBlock>()?.clone();
            let mut root = self.root.borrow_mut();
            let path_depth = self.path.len();
            let (target, segment) = visit::VisitPath::find_parent(self.path.clone(), &mut root)?;
            let Segment::Block { ident, labels, nth } = &segment else {
                return Err(HclError::InvalidType {
                    expected: "block",
                    actual: segment.kind_name(),
                })?;
            };
            let body: &mut Body = match target {
                Target::Body(body) => body,
                Target::Block(parent) => &mut parent.body,
                _ => Err(HclError::InvalidType {
                    expected: "a body or block",
                    actual: target.type_name(),
                })?,
            };
            let depth = path_depth.saturating_sub(1);
            let idx = Self::find_block_index(body, ident, labels, *nth).ok_or_else(|| {
                HclError::NotFound {
                    path: self.parent_path(),
                    segment: segment.clone(),
                }
            })?;
            // Keep the replaced block's own indentation.
            let old = body.iter().nth(idx).cloned();
            let indent = Self::structure_indent(old.clone()).unwrap_or_else(|| "  ".repeat(depth));
            let mut block = Self::block_at_indent(&builder, &indent)?;
            // Without a comment of its own the builder keeps the original's
            // leading comments and blank lines.
            let new_prefix = block.decor().prefix().map(|p| p.to_string()).unwrap_or_default();
            if !new_prefix.contains(['#', '/'])
                && let Some(p) = old.as_ref().and_then(|s| s.decor().prefix())
            {
                block.decor_mut().set_prefix(p.to_string());
            }
            body.remove(idx);
            body.insert(idx, block);
            return Ok(());
        }

        let mut root = self.root.borrow_mut();
        let (target, segment) = visit::VisitPath::find_parent(self.path.clone(), &mut root)?;

        let value = lua_to_hcl(lua, value)?;

        match target {
            Target::Body(body) => {
                let key = segment.as_str().ok_or(HclError::InvalidType {
                    expected: "key",
                    actual: "index",
                })?;
                if let Some(mut attr) = body.get_attribute_mut(key) {
                    *(attr.value_mut()) = value;
                    Ok(())
                } else {
                    Err(HclError::NotFound {
                        path: self.parent_path(),
                        segment,
                    })?
                }
            }
            Target::Block(block) => {
                let key = segment.as_str().ok_or(HclError::InvalidType {
                    expected: "key",
                    actual: "index",
                })?;
                if let Some(mut attr) = block.body.get_attribute_mut(key) {
                    *(attr.value_mut()) = value;
                    Ok(())
                } else {
                    Err(HclError::NotFound {
                        path: self.parent_path(),
                        segment,
                    })?
                }
            }
            Target::Object(object) => {
                let key = segment.as_str().ok_or(HclError::InvalidType {
                    expected: "key",
                    actual: "index",
                })?;
                if let Some(object_value) =
                    find_object_key(object, key).and_then(|k| object.get_mut(&k))
                {
                    *object_value.expr_mut() = value;
                    Ok(())
                } else {
                    Err(HclError::NotFound {
                        path: self.parent_path(),
                        segment,
                    })?
                }
            }
            Target::Array(array) => {
                let index = segment.as_index().ok_or(HclError::InvalidType {
                    expected: "index",
                    actual: "key",
                })?;
                if let Some(expr) = array.get_mut(index) {
                    *expr = value;
                    Ok(())
                } else {
                    Err(HclError::IndexOutOfBounds {
                        index: index as i64 + 1,
                        len: array.len(),
                    })?
                }
            }
            _ => Err(HclError::InvalidType {
                expected: "a body, block, object or array",
                actual: target.type_name(),
            })?,
        }
    }

    // ---- exists / type checks ----------------------------------------

    fn exists(&self) -> bool {
        if self.path.is_empty() {
            return true;
        }
        let mut root = self.root.borrow_mut();
        visit::VisitPath::find(self.path.clone(), &mut root).is_ok()
    }

    fn attr_keys_from_body(body: &Body) -> Vec<String> {
        body.attributes()
            .map(|attr| attr.key.as_str().to_string())
            .collect()
    }

    fn attribute_keys(&self) -> Result<Vec<String>> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Body(body) => Ok(Self::attr_keys_from_body(body)),
            Target::Block(block) => Ok(Self::attr_keys_from_body(&block.body)),
            Target::Object(object) => Ok(object_keys(object)),
            _ => Err(HclError::InvalidType {
                expected: "a body, block or object",
                actual: target.type_name(),
            })?,
        }
    }

    fn len(&self) -> Result<i64> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Body(body) => Ok((body.len()) as i64),
            Target::Block(block) => Ok((block.body.len()) as i64),
            Target::Object(object) => Ok(object.len() as i64),
            Target::Array(array) => Ok(array.len() as i64),
            _ => Err(HclError::InvalidType {
                expected: "a body, block, object or array",
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
    /// at index `nth` (default 0) from `body`. Returns `false` if no such
    /// block exists.
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

    fn remove(&self) -> Result<()> {
        let mut root = self.root.borrow_mut();
        let (target, segment) = visit::VisitPath::find_parent(self.path.clone(), &mut root)?;

        match target {
            Target::Body(body) => {
                if let Segment::Block { ident, labels, nth } = &segment {
                    return if Self::remove_block_from_body(body, ident, labels, *nth) {
                        Ok(())
                    } else {
                        Err(HclError::NotFound {
                            path: self.parent_path(),
                            segment,
                        })?
                    };
                }
                let key = segment.as_str().ok_or(HclError::InvalidType {
                    expected: "a key or block",
                    actual: segment.kind_name(),
                })?;
                remove_named(body, key, self.parent_path())
            }
            Target::Block(block) => {
                if let Segment::Block { ident, labels, nth } = &segment {
                    return if Self::remove_block_from_body(&mut block.body, ident, labels, *nth) {
                        Ok(())
                    } else {
                        Err(HclError::NotFound {
                            path: self.parent_path(),
                            segment,
                        })?
                    };
                }
                let key = segment.as_str().ok_or(HclError::InvalidType {
                    expected: "a key or block",
                    actual: segment.kind_name(),
                })?;
                remove_named(&mut block.body, key, self.parent_path())
            }
            Target::Object(object) => {
                let key = segment.as_str().ok_or(HclError::InvalidType {
                    expected: "key",
                    actual: "index",
                })?;
                if let Some(obj_key) = find_object_key(object, key) {
                    object.remove(&obj_key);
                    Ok(())
                } else {
                    Err(HclError::NotFound {
                        path: self.parent_path(),
                        segment,
                    })?
                }
            }
            Target::Array(array) => {
                let index = segment.as_index().ok_or(HclError::InvalidType {
                    expected: "index",
                    actual: "key",
                })?;
                if index < array.len() {
                    array.remove(index);
                    Ok(())
                } else {
                    Err(HclError::IndexOutOfBounds {
                        index: index as i64 + 1,
                        len: array.len(),
                    })?
                }
            }
            _ => Err(HclError::InvalidType {
                expected: "a body, block, object or array",
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

    /// Build an `hcl.block(…)` builder into an edit-CST block laid out at
    /// `indent` (its inner body rendered one step deeper, its own header
    /// prefixed with `indent`). Shared by `add`/`write` so both indent a
    /// spliced-in block identically.
    fn block_at_indent(
        builder: &super::HclBlock,
        indent: &str,
    ) -> Result<hcl::edit::structure::Block> {
        let mut block = builder.to_edit_block(indent.len() / 2)?;
        // A builder comment already sits (indented) in the parsed prefix;
        // only a comment-less block gets the plain indent.
        let has_comment = block
            .decor()
            .prefix()
            .is_some_and(|p| p.to_string().contains(['#', '/']));
        if !has_comment {
            block.decor_mut().set_prefix(indent.to_string());
        }
        Ok(block)
    }

    /// Set a structure's leading-whitespace prefix to `indent`.
    fn indented(item: impl Into<Structure>, indent: &str) -> Structure {
        let mut s = item.into();
        s.decor_mut().set_prefix(indent.to_string());
        s
    }

    /// Absolute structure index at which a new attribute keyed `key` should
    /// land for the given 1-based "attribute position" `pos`: `usize::MAX`
    /// means append. Counts only attributes (blocks are skipped) when
    /// walking to `pos`, so the index lines up with what a script sees via
    /// `:attribute_keys()`.
    fn attr_structure_index(body: &Body, pos: usize) -> usize {
        if pos == usize::MAX {
            return body.len();
        }
        let mut seen = 0usize;
        for (idx, structure) in body.iter().enumerate() {
            if structure.is_attribute() {
                if seen == pos {
                    return idx;
                }
                seen += 1;
            }
        }
        body.len()
    }

    fn add_kv_body(
        body: &mut Body,
        depth: usize,
        at: usize,
        key: &str,
        value: Expression,
    ) -> Result<()> {
        if body.has_attribute(key) {
            return Err(mlua::Error::runtime(format!("duplicate key: {key}")));
        }
        let ident = Ident::try_new(key).map_err(|_| {
            mlua::Error::runtime(format!(
                "invalid attribute name {key:?}: not an HCL identifier"
            ))
        })?;
        let indent = Self::body_indent(body, depth);
        let attr = Self::indented(Attribute::new(ident, value), &indent);
        if at >= body.len() {
            body.push(attr);
        } else {
            body.insert(at, attr);
        }
        Ok(())
    }

    fn add_kv(&self, lua: &Lua, at: Option<usize>, key: &str, value: Value) -> Result<()> {
        let mut root = self.root.borrow_mut();
        let depth = self.path.len();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;
        let value = lua_to_hcl(lua, value)?;

        match target {
            Target::Body(body) => {
                let idx = at.map_or(usize::MAX, |p| Self::attr_structure_index(body, p));
                Self::add_kv_body(body, depth, idx, key, value)
            }
            Target::Block(block) => {
                let idx = at.map_or(usize::MAX, |p| Self::attr_structure_index(&block.body, p));
                Self::add_kv_body(&mut block.body, depth + 1, idx, key, value)
            }
            Target::Object(object) => {
                if find_object_key(object, key).is_some() {
                    return Err(mlua::Error::runtime(format!("duplicate key: {key}")));
                }
                // lay the new entry out like the last one (indent, newline)
                let mut new_key = new_object_key(key);
                let mut terminator = None;
                if let Some((k, v)) = object.iter().last() {
                    if let Some(p) = k.decor().prefix() {
                        new_key.decor_mut().set_prefix(p.to_string());
                    }
                    terminator = Some(v.terminator());
                }
                let mut ov = hcl::edit::expr::ObjectValue::new(value);
                if let Some(t) = terminator {
                    ov.set_terminator(t);
                }
                object.insert(new_key, ov);
                Ok(())
            }
            _ => Err(HclError::InvalidType {
                expected: "a body, block or object",
                actual: target.type_name(),
            })?,
        }
    }

    /// `index` is 1-based, matching Lua's own indexing convention;
    /// out-of-range (including `<= 0`) clamps to the nearest end rather
    /// than erroring.
    fn add_index(&self, lua: &Lua, index: i64, value: Value) -> Result<()> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;
        let value = lua_to_hcl(lua, value)?;

        match target {
            Target::Array(array) => {
                let idx = if index < 1 {
                    0usize
                } else {
                    ((index - 1) as usize).min(array.len())
                };
                array.insert(idx, value);
                Ok(())
            }
            _ => Err(HclError::InvalidType {
                expected: "an array",
                actual: target.type_name(),
            })?,
        }
    }

    fn add_value(&self, lua: &Lua, value: Value) -> Result<()> {
        // A builder block (`hcl.block(..)`) appends a nested block to a
        // body/block target, indented to match its siblings.
        if let Value::UserData(ud) = &value
            && ud.is::<super::HclBlock>()
        {
            let builder = ud.borrow::<super::HclBlock>()?.clone();
            let mut root = self.root.borrow_mut();
            let path_depth = self.path.len();
            let target = visit::VisitPath::find(self.path.clone(), &mut root)?;
            let (body, depth): (&mut Body, usize) = match target {
                Target::Body(body) => (body, path_depth),
                Target::Block(parent) => (&mut parent.body, path_depth + 1),
                _ => Err(HclError::InvalidType {
                    expected: "a body or block",
                    actual: target.type_name(),
                })?,
            };
            let indent = Self::body_indent(body, depth);
            let mut block = Self::block_at_indent(&builder, &indent)?;
            // a blank line before the block, unless it opens the body
            if !body.is_empty() {
                let prefix = block.decor().prefix().map(|p| p.to_string()).unwrap_or_default();
                block.decor_mut().set_prefix(format!("\n{prefix}"));
            }
            body.push(block);
            return Ok(());
        }

        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;
        let value = lua_to_hcl(lua, value)?;

        match target {
            Target::Array(array) => {
                array.push(value);
                Ok(())
            }
            _ => Err(HclError::InvalidType {
                expected: "an array",
                actual: target.type_name(),
            })?,
        }
    }

    /// `add(value)` (array append / block append), `add(key, value)`
    /// (body/block/object member — errors on a duplicate key).
    fn add(&self, lua: &Lua, args: Variadic<Value>) -> Result<()> {
        match args.len() {
            1 => self.add_value(lua, args.into_iter().next().unwrap()),
            2 => {
                let mut it = args.into_iter();
                let first = it.next().unwrap();
                let value = it.next().unwrap();
                match first {
                    Value::Integer(i) => self.add_index(lua, i, value),
                    Value::String(s) => self.add_kv(lua, None, &s.to_str()?, value),
                    other => Err(mlua::Error::runtime(format!(
                        "add: first argument must be an index or key, got {}",
                        other.type_name()
                    ))),
                }
            }
            n => Err(mlua::Error::runtime(format!(
                "add: expected 1 or 2 arguments, got {n}"
            ))),
        }
    }

    /// `add_at(index, value)` (array insert, clamped), `add_at(index, key,
    /// value)` (body/block/object member at a given attribute position —
    /// errors on a duplicate key). `index` is 1-based.
    fn add_at(&self, lua: &Lua, args: Variadic<Value>) -> Result<()> {
        let mut it = args.into_iter();
        let index = match it.next() {
            Some(Value::Integer(i)) => i,
            other => {
                return Err(mlua::Error::runtime(format!(
                    "add_at: first argument must be an integer index, got {}",
                    other.map_or("nothing", |v| v.type_name())
                )));
            }
        };
        let rest: Vec<Value> = it.collect();
        match rest.len() {
            1 => self.add_index(lua, index, rest.into_iter().next().unwrap()),
            2 => {
                let mut it = rest.into_iter();
                let key = match it.next().unwrap() {
                    Value::String(s) => s.to_str()?.to_string(),
                    other => {
                        return Err(mlua::Error::runtime(format!(
                            "add_at: key must be a string, got {}",
                            other.type_name()
                        )));
                    }
                };
                let value = it.next().unwrap();
                let pos = zero_based(index)?;
                self.add_kv(lua, Some(pos), &key, value)
            }
            n => Err(mlua::Error::runtime(format!(
                "add_at: expected 2 or 3 arguments, got {}",
                n + 1
            ))),
        }
    }

    /// Read-modify-write a single attribute's value via `update_fn`.
    /// `update_fn` receives the current decoded value and returns the new
    /// one, or `nil` to remove the attribute. Requires the target to be a
    /// scalar-decodable attribute value (`read`'s own restriction) — a
    /// block/body/object/array target raises.
    fn update_attr(&self, lua: &Lua, update_fn: mlua::Function) -> Result<()> {
        match self.compute_update(lua, &update_fn)? {
            Some(next) => self.write(lua, next),
            None => self.remove(),
        }
    }

    /// Call `update_fn` with the current value (a computed expression is
    /// handed over as a raw `HclExpr`, which writes back verbatim) and return
    /// the new value, `None` meaning "remove". Does not modify anything.
    fn compute_update(&self, lua: &Lua, update_fn: &mlua::Function) -> Result<Option<Value>> {
        let current = match self.read(lua) {
            Ok(v) => v,
            Err(e) => match self.raw() {
                Ok(expr) => Value::UserData(lua.create_userdata(expr)?),
                Err(_) => return Err(e),
            },
        };
        let next = update_fn
            .call::<Value>(current)
            .map_err(|e| mlua::Error::external(CallbackFailed(e)))?;
        Ok(if matches!(next, Value::Nil) { None } else { Some(next) })
    }

    /// `update(update_fn)`: self-update, only valid on a scalar attribute
    /// value. `nil` returned by `update_fn` removes the attribute.
    fn update_self(&self, lua: &Lua, update_fn: mlua::Function) -> Result<()> {
        self.update_attr(lua, update_fn)
    }

    fn key_at(body: &Body, pos: usize) -> Option<String> {
        let mut seen = 0usize;
        for structure in body.iter() {
            if let Some(attr) = structure.as_attribute() {
                if seen == pos {
                    return Some(attr.key.as_str().to_string());
                }
                seen += 1;
            }
        }
        None
    }

    /// `update(key, update_fn)` / `update(index, update_fn)` (array) /
    /// `update_at(index, update_fn)` (map, by attribute position).
    fn update_named(&self, lua: &Lua, at: Segment, update_fn: mlua::Function) -> Result<()> {
        self.traverse(at).update_attr(lua, update_fn)
    }

    fn update_at_map(&self, lua: &Lua, index: i64, update_fn: mlua::Function) -> Result<()> {
        let pos = zero_based(index)?;
        let key = {
            let mut root = self.root.borrow_mut();
            let target = visit::VisitPath::find(self.path.clone(), &mut root)?;
            let body = match target {
                Target::Body(body) => body,
                Target::Block(block) => &mut block.body,
                _ => Err(HclError::InvalidType {
                    expected: "a body or block",
                    actual: target.type_name(),
                })?,
            };
            Self::key_at(body, pos).ok_or(HclError::IndexOutOfBounds {
                index,
                len: body.attributes().count(),
            })?
        };
        self.update_named(lua, Segment::Attr { key }, update_fn)
    }

    /// `update_all(update_fn)`: applies `update_fn` to every attribute
    /// value of a body/block, in place. A `nil` return removes that
    /// attribute. Skips nested blocks — only attribute values participate.
    fn update_all(&self, lua: &Lua, update_fn: mlua::Function) -> Result<()> {
        // Two phases, so a failing callback changes nothing: compute every
        // new value first, then apply.
        let keys = self.attribute_keys()?;
        let mut planned = Vec::with_capacity(keys.len());
        for key in keys {
            let next = self.attr(&key).compute_update(lua, &update_fn)?;
            planned.push((key, next));
        }
        for (key, next) in planned {
            let child = self.attr(&key);
            match next {
                Some(v) => child.write(lua, v)?,
                None => child.remove()?,
            }
        }
        Ok(())
    }

    /// `upsert(key, upsert_fn)`: like `update`, but a missing key calls
    /// `upsert_fn(nil)` and adds the result instead of raising.
    fn upsert(&self, lua: &Lua, key: &str, upsert_fn: mlua::Function) -> Result<()> {
        let child = self.attr(key);
        if child.exists() {
            child.update_attr(lua, upsert_fn)
        } else {
            let next = upsert_fn
                .call::<Value>(Value::Nil)
                .map_err(|e| mlua::Error::external(CallbackFailed(e)))?;
            if matches!(next, Value::Nil) {
                Ok(())
            } else {
                self.add_kv(lua, None, key, next)
            }
        }
    }

    /// `upsert_at(index, key, upsert_fn)`: as `upsert`, but a newly-added
    /// entry lands at the given attribute position rather than the end.
    fn upsert_at(&self, lua: &Lua, index: i64, key: &str, upsert_fn: mlua::Function) -> Result<()> {
        let child = self.attr(key);
        if child.exists() {
            child.update_attr(lua, upsert_fn)
        } else {
            let next = upsert_fn
                .call::<Value>(Value::Nil)
                .map_err(|e| mlua::Error::external(CallbackFailed(e)))?;
            if matches!(next, Value::Nil) {
                Ok(())
            } else {
                let pos = zero_based(index)?;
                self.add_kv(lua, Some(pos), key, next)
            }
        }
    }

    /// The `update` dispatch: `update(update_fn)` (self), `update(key,
    /// update_fn)` (map), `update(index, update_fn)` (array/map-by-position
    /// — both accepted, matching `remove`'s dual `remove(index)`/
    /// `remove_at(index)` spelling for the same reason).
    fn update_dispatch(&self, lua: &Lua, args: Variadic<Value>) -> Result<()> {
        let mut it = args.into_iter();
        let first = it
            .next()
            .ok_or_else(|| mlua::Error::runtime("update: expected 1 or 2 arguments, got 0"))?;
        match it.next() {
            None => {
                let update_fn: mlua::Function = mlua::FromLua::from_lua(first, lua)?;
                self.update_self(lua, update_fn)
            }
            Some(second) => {
                let update_fn: mlua::Function = mlua::FromLua::from_lua(second, lua)?;
                match first {
                    // a position: an array element, or a body's n-th attribute
                    Value::Integer(i) => self.update_at_dispatch(lua, i, update_fn),
                    Value::String(s) => self.update_named(
                        lua,
                        Segment::Attr {
                            key: s.to_str()?.to_string(),
                        },
                        update_fn,
                    ),
                    other => Err(mlua::Error::runtime(format!(
                        "update: first argument must be an index or key, got {}",
                        other.type_name()
                    ))),
                }
            }
        }
    }

    /// `update_at(index, update_fn)`: array element or map attribute by
    /// position.
    fn update_at_dispatch(&self, lua: &Lua, index: i64, update_fn: mlua::Function) -> Result<()> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;
        let is_array = matches!(target, Target::Array(_));
        drop(root);
        if is_array {
            let pos = zero_based(index)?;
            self.update_named(lua, Segment::Index { index: pos }, update_fn)
        } else {
            self.update_at_map(lua, index, update_fn)
        }
    }

    /// The `remove` dispatch: `remove()` uses the cursor's own path,
    /// `remove(index)` removes an array element, `remove(key)` removes a
    /// body/block/object member.
    fn remove_dispatch(&self, args: Variadic<Value>) -> Result<()> {
        match args.len() {
            0 => self.remove(),
            1 => match args.into_iter().next().unwrap() {
                Value::Integer(i) => self.remove_index(i),
                Value::String(s) => self.remove_key(&s.to_str()?),
                other => Err(mlua::Error::runtime(format!(
                    "remove: argument must be an index or key, got {}",
                    other.type_name()
                ))),
            },
            n => Err(mlua::Error::runtime(format!(
                "remove: expected 0 or 1 arguments, got {n}"
            ))),
        }
    }

    /// `remove_at(index)`: array element by position, or map attribute by
    /// attribute position (kept as a distinct spelling from `remove(key)`
    /// rather than guessed from argument type).
    fn remove_at(&self, index: i64) -> Result<()> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;
        match target {
            Target::Array(_) => {
                drop(root);
                self.remove_index(index)
            }
            Target::Body(body) => {
                let pos = zero_based(index)?;
                let key = Self::key_at(body, pos).ok_or(HclError::IndexOutOfBounds {
                    index,
                    len: body.attributes().count(),
                })?;
                body.remove_attribute(&key);
                Ok(())
            }
            Target::Block(block) => {
                let pos = zero_based(index)?;
                let key = Self::key_at(&block.body, pos).ok_or(HclError::IndexOutOfBounds {
                    index,
                    len: block.body.attributes().count(),
                })?;
                block.body.remove_attribute(&key);
                Ok(())
            }
            _ => Err(HclError::InvalidType {
                expected: "a body, block or array",
                actual: target.type_name(),
            })?,
        }
    }

    fn remove_key(&self, key: &str) -> Result<()> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Body(body) => remove_named(body, key, self.path.clone()),
            Target::Block(block) => remove_named(&mut block.body, key, self.path.clone()),
            Target::Object(object) => {
                if let Some(obj_key) = find_object_key(object, key) {
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
                expected: "a body, block or object",
                actual: target.type_name(),
            })?,
        }
    }

    /// `index` is 1-based, matching Lua's own indexing convention.
    fn remove_index(&self, index: i64) -> Result<()> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Array(array) => {
                if index >= 1 && ((index - 1) as usize) < array.len() {
                    array.remove((index - 1) as usize);
                    Ok(())
                } else {
                    Err(HclError::IndexOutOfBounds {
                        index,
                        len: array.len(),
                    })?
                }
            }
            _ => Err(HclError::InvalidType {
                expected: "an array",
                actual: target.type_name(),
            })?,
        }
    }

    fn block_types_from_body(body: &Body) -> Vec<String> {
        let mut seen = Vec::new();
        for block in body.blocks() {
            let ident = block.ident.as_str().to_string();
            if !seen.contains(&ident) {
                seen.push(ident);
            }
        }
        seen
    }

    fn block_types(&self) -> Result<Vec<String>> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Body(body) => Ok(Self::block_types_from_body(body)),
            Target::Block(block) => Ok(Self::block_types_from_body(&block.body)),
            _ => Err(HclError::InvalidType {
                expected: "a body or block",
                actual: target.type_name(),
            })?,
        }
    }

    fn block_labels_from_body(body: &Body, ident: &str) -> Vec<Vec<String>> {
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
        seen
    }

    fn block_labels(&self, ident: &str) -> Result<Vec<Vec<String>>> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Body(body) => Ok(Self::block_labels_from_body(body, ident)),
            Target::Block(block) => Ok(Self::block_labels_from_body(&block.body, ident)),
            _ => Err(HclError::InvalidType {
                expected: "a body or block",
                actual: target.type_name(),
            })?,
        }
    }

    fn blocks_from_body(
        &self,
        body: &Body,
        filter_ident: Option<&str>,
        filter_labels: &[String],
    ) -> Vec<HclDoc> {
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
                hcl
            })
            .collect()
    }

    fn resolve_and_collect_blocks(
        &self,
        filter_ident: Option<&str>,
        filter_labels: &[String],
    ) -> Result<Vec<HclDoc>> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Body(body) => Ok(self.blocks_from_body(body, filter_ident, filter_labels)),
            Target::Block(block) => {
                Ok(self.blocks_from_body(&block.body, filter_ident, filter_labels))
            }
            _ => Err(HclError::InvalidType {
                expected: "a body or block",
                actual: target.type_name(),
            })?,
        }
    }

    /// `blocks()` (all), or `blocks(ident, [labels...])` (filtered), via
    /// variadic parsing.
    fn blocks(&self, args: Variadic<Value>) -> Result<Vec<HclDoc>> {
        if args.is_empty() {
            return self.resolve_and_collect_blocks(None, &Self::any_labels());
        }
        let mut it = args.into_iter();
        let ident = match it.next().unwrap() {
            Value::String(s) => s.to_str()?.to_string(),
            other => {
                return Err(mlua::Error::runtime(format!(
                    "blocks: ident must be a string, got {}",
                    other.type_name()
                )));
            }
        };
        let rest: Vec<Value> = it.collect();
        let labels = Self::parse_filter_labels(&rest)?;
        self.resolve_and_collect_blocks(Some(&ident), &labels)
    }

    fn attributes_from_body(&self, body: &Body) -> Vec<HclDoc> {
        body.attributes()
            .map(|attr| {
                let mut new = self.clone();
                new.path.push(Segment::Attr {
                    key: attr.key.as_str().to_string(),
                });
                new
            })
            .collect()
    }

    fn attributes(&self) -> Result<Vec<HclDoc>> {
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Body(body) => Ok(self.attributes_from_body(body)),
            Target::Block(block) => Ok(self.attributes_from_body(&block.body)),
            Target::Object(object) => Ok(object_keys(object)
                .into_iter()
                .map(|key| self.attr(&key))
                .collect()),
            _ => Err(HclError::InvalidType {
                expected: "a body, block or object",
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

    /// `block_count(ident)` / `block_count(ident, label...)` /
    /// `block_count(ident, [labels])`, via variadic parsing.
    fn block_count(&self, args: Variadic<Value>) -> Result<i64> {
        let mut it = args.into_iter();
        let ident = match it.next() {
            Some(Value::String(s)) => s.to_str()?.to_string(),
            _ => {
                return Err(mlua::Error::runtime("block_count: ident must be a string"));
            }
        };
        let rest: Vec<Value> = it.collect();
        let labels = Self::parse_filter_labels(&rest)?;
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;

        match target {
            Target::Body(body) => Ok(Self::block_count_from_body(body, ident.as_str(), &labels)),
            Target::Block(block) => Ok(Self::block_count_from_body(
                &block.body,
                ident.as_str(),
                &labels,
            )),
            _ => Err(HclError::InvalidType {
                expected: "a body or block",
                actual: target.type_name(),
            })?,
        }
    }

    fn is_array(&self) -> bool {
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

    fn is_object(&self) -> bool {
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

    fn is_expr(&self) -> bool {
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

    fn is_block(&self) -> bool {
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

    fn is_body(&self) -> bool {
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

    /// The resolved node's kind, as the Luau surface names it. Reads the
    /// same `Target` variant the `is_*` predicates match on, so the two
    /// can never disagree.
    fn kind(&self) -> Result<&'static str> {
        if self.path.is_empty() {
            return Ok("body");
        }
        let mut root = self.root.borrow_mut();
        let target = visit::VisitPath::find(self.path.clone(), &mut root)?;
        Ok(match target {
            Target::Body(_) => "body",
            Target::Block(_) => "block",
            Target::Object(_) => "object",
            Target::Array(_) => "array",
            Target::Expr(_) => "expr",
        })
    }
}

/// Traversal via bracket syntax: `h["key"]` (string) or `h[0]` (index).
/// Registered as the fallback `__index`; named methods (`read`, `block`,
/// …) are looked up first by mlua's generated `__index`.
fn index(h: &HclDoc, key: Value) -> Result<HclDoc> {
    match key {
        Value::String(s) => Ok(h.traverse_str(&s.to_str()?)),
        Value::Integer(i) => h.traverse_index(i),
        other => Err(mlua::Error::runtime(format!(
            "hcl.edit index must be a string key or integer index, got {}",
            other.type_name()
        ))),
    }
}

impl UserData for HclDoc {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // Bracket-syntax traversal (fallback after named-method lookup).
        methods.add_meta_method(MetaMethod::Index, |_, h, key: Value| index(h, key));

        // Traversal navigators.
        methods.add_method("attr", |_, h, key: String| Ok(h.attr(&key)));
        methods.add_method("attribute", |_, h, key: String| Ok(h.attr(&key)));

        methods.add_method("block", |_, h, args: Variadic<Value>| {
            let mut it = args.into_iter();
            let ident = match it.next() {
                Some(Value::String(s)) => s.to_str()?.to_string(),
                _ => return Err(mlua::Error::runtime("block: ident must be a string")),
            };
            let rest: Vec<Value> = it.collect();
            let (labels, nth) = HclDoc::parse_labels_and_nth(&rest)?;
            Ok(h.traverse_block(ident, labels, nth))
        });

        methods.add_method("exists", |_, h, ()| Ok(h.exists()));

        methods.add_method("attrs", |_, h, ()| h.attributes());
        methods.add_method("attributes", |_, h, ()| h.attributes());
        methods.add_method("try_attrs", |_, h, ()| Ok(h.attributes().ok()));
        methods.add_method("try_attributes", |_, h, ()| Ok(h.attributes().ok()));

        methods.add_method("attr_keys", |_, h, ()| h.attribute_keys());
        methods.add_method("attribute_keys", |_, h, ()| h.attribute_keys());
        methods.add_method("try_attr_keys", |_, h, ()| Ok(h.attribute_keys().ok()));
        methods.add_method("try_attribute_keys", |_, h, ()| Ok(h.attribute_keys().ok()));

        methods.add_method("blocks", |_, h, args: Variadic<Value>| h.blocks(args));
        methods.add_method("try_blocks", |_, h, args: Variadic<Value>| {
            Ok(h.blocks(args).ok())
        });

        methods.add_method("block_types", |_, h, ()| h.block_types());
        methods.add_method("try_block_types", |_, h, ()| Ok(h.block_types().ok()));

        methods.add_method("block_labels", |_, h, ident: String| h.block_labels(&ident));
        methods.add_method("try_block_labels", |_, h, ident: String| {
            Ok(h.block_labels(&ident).ok())
        });

        methods.add_method("block_count", |_, h, args: Variadic<Value>| {
            h.block_count(args)
        });
        methods.add_method("try_block_count", |_, h, args: Variadic<Value>| {
            Ok(h.block_count(args).ok())
        });

        // Type checks.
        methods.add_method("is_array", |_, h, ()| Ok(h.is_array()));
        methods.add_method("is_object", |_, h, ()| Ok(h.is_object()));
        methods.add_method("is_expr", |_, h, ()| Ok(h.is_expr()));
        methods.add_method("is_block", |_, h, ()| Ok(h.is_block()));
        methods.add_method("is_body", |_, h, ()| Ok(h.is_body()));

        // Read / modify.
        methods.add_method("read", |lua, h, ()| h.read(lua));
        methods.add_method("try_read", |lua, h, ()| Ok(h.read(lua).ok()));
        methods.add_method("raw", |_, h, ()| h.raw());
        methods.add_method("try_raw", |_, h, ()| Ok(h.raw().ok()));

        methods.add_method("write", |lua, h, value: Value| h.write(lua, value));
        methods.add_method("try_write", |lua, h, value: Value| {
            Ok(h.write(lua, value).is_ok())
        });

        methods.add_method("len", |_, h, ()| h.len());
        methods.add_method("try_len", |_, h, ()| Ok(h.len().ok()));

        methods.add_method("kind", |_, h, ()| h.kind());
        methods.add_method("try_kind", |_, h, ()| Ok(h.kind().ok()));

        // Structural edits.
        methods.add_method("add", |lua, h, args: Variadic<Value>| h.add(lua, args));
        methods.add_method("try_add", |lua, h, args: Variadic<Value>| {
            Ok(h.add(lua, args).is_ok())
        });

        methods.add_method("add_at", |lua, h, args: Variadic<Value>| {
            h.add_at(lua, args)
        });
        methods.add_method("try_add_at", |lua, h, args: Variadic<Value>| {
            Ok(h.add_at(lua, args).is_ok())
        });

        methods.add_method("update", |lua, h, args: Variadic<Value>| {
            h.update_dispatch(lua, args)
        });
        methods.add_method("try_update", |lua, h, args: Variadic<Value>| {
            attempt(h.update_dispatch(lua, args))
        });

        methods.add_method(
            "update_at",
            |lua, h, (index, update_fn): (i64, mlua::Function)| {
                h.update_at_dispatch(lua, index, update_fn)
            },
        );
        methods.add_method(
            "try_update_at",
            |lua, h, (index, update_fn): (i64, mlua::Function)| {
                attempt(h.update_at_dispatch(lua, index, update_fn))
            },
        );

        methods.add_method("update_all", |lua, h, update_fn: mlua::Function| {
            h.update_all(lua, update_fn)
        });
        methods.add_method("try_update_all", |lua, h, update_fn: mlua::Function| {
            attempt(h.update_all(lua, update_fn))
        });

        methods.add_method(
            "upsert",
            |lua, h, (key, upsert_fn): (String, mlua::Function)| h.upsert(lua, &key, upsert_fn),
        );
        methods.add_method(
            "try_upsert",
            |lua, h, (key, upsert_fn): (String, mlua::Function)| {
                attempt(h.upsert(lua, &key, upsert_fn))
            },
        );

        methods.add_method(
            "upsert_at",
            |lua, h, (index, key, upsert_fn): (i64, String, mlua::Function)| {
                h.upsert_at(lua, index, &key, upsert_fn)
            },
        );
        methods.add_method(
            "try_upsert_at",
            |lua, h, (index, key, upsert_fn): (i64, String, mlua::Function)| {
                attempt(h.upsert_at(lua, index, &key, upsert_fn))
            },
        );

        methods.add_method("remove", |_, h, args: Variadic<Value>| {
            h.remove_dispatch(args)
        });
        methods.add_method("try_remove", |_, h, args: Variadic<Value>| {
            Ok(h.remove_dispatch(args).is_ok())
        });

        methods.add_method("remove_at", |_, h, index: i64| h.remove_at(index));
        methods.add_method("try_remove_at", |_, h, index: i64| {
            Ok(h.remove_at(index).is_ok())
        });

        methods.add_method("to_string", |_, h, ()| h.to_string());
        methods.add_method("try_to_string", |_, h, ()| Ok(h.to_string().ok()));
        methods.add_meta_method(MetaMethod::ToString, |_, h, ()| h.to_string());
    }
}
