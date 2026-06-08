//! Path resolution over a single `hcl::edit` `Body`. Walks a `Vec<Segment>`
//! to a mutable `Target`. Ported from the `hrs` crate, adapted to a single
//! root body (no multi-document loop).

use super::types::{HclError, Segment, Target, labels_match};
use hcl::edit::Ident;
use hcl::edit::expr::{Expression, ObjectKey};
use hcl::edit::structure::{Block, Body};

pub(crate) struct VisitPath {
    path: Vec<Segment>,
    visited: usize,
}

impl VisitPath {
    pub fn find(path: Vec<Segment>, root: &mut Body) -> Result<Target<'_>, HclError> {
        Self { path, visited: 0 }.visit_body(root)
    }

    pub fn find_parent(
        mut path: Vec<Segment>,
        root: &mut Body,
    ) -> Result<(Target<'_>, Segment), HclError> {
        let seg = path.pop().ok_or(HclError::NotOnRoot)?;
        Self { path, visited: 0 }.visit_body(root).map(|t| (t, seg))
    }

    fn next(&self) -> Option<Segment> {
        self.path.get(self.visited).cloned()
    }

    fn done(&self) -> bool {
        self.path.len() == self.visited
    }

    fn not_found(&self, seg: Segment) -> HclError {
        HclError::NotFound {
            path: self.path_traversed(),
            segment: seg,
        }
    }

    fn path_traversed(&self) -> Vec<Segment> {
        self.path[..self.visited].to_vec()
    }

    fn visit_body<'a>(&mut self, node: &'a mut Body) -> Result<Target<'a>, HclError> {
        let Some(seg) = self.next() else {
            return Ok(Target::Body(node));
        };
        self.visited += 1;

        match &seg {
            Segment::Text { text } => {
                let key = text.clone();

                let has_attr = node.has_attribute(&key);
                let has_block = node
                    .blocks()
                    .any(|block| block.labels.is_empty() && block.ident.value().as_str() == key);

                if has_attr && has_block {
                    return Err(HclError::AmbiguousKey { key });
                }

                if let Some(mut attr) = node.get_attribute_mut(&key) {
                    // SAFETY: the AttributeMut borrows from `node` with lifetime
                    // 'a; value_mut() yields &mut Expression. We round-trip through
                    // a raw pointer to extend the lifetime to 'a. Sound because the
                    // attribute lives in `node` ('a), we return immediately (no
                    // aliasing), and the guard's drop leaves the data valid.
                    let expr_ptr: *mut Expression = attr.value_mut();
                    return self.visit_expr(unsafe { &mut *expr_ptr });
                }

                if let Some(block) = node
                    .blocks_mut()
                    .find(|block| block.labels.is_empty() && block.ident.value().as_str() == key)
                {
                    return self.visit_block(block);
                }

                Err(self.not_found(seg))
            }
            Segment::Attr { key } => {
                if let Some(mut attr) = node.get_attribute_mut(key) {
                    let expr_ptr: *mut Expression = attr.value_mut();
                    return self.visit_expr(unsafe { &mut *expr_ptr });
                }
                Err(self.not_found(seg))
            }
            Segment::Block { ident, labels, nth } => {
                self.visit_block_segment(node, ident, labels, *nth, seg.clone())
            }
            Segment::Index { .. } => Err(HclError::InvalidType {
                expected: "Text, Attr, or Block segment",
                actual: "Index segment",
            }),
        }
    }

    fn visit_block_segment<'a>(
        &mut self,
        node: &'a mut Body,
        ident: &str,
        labels: &[String],
        nth: Option<usize>,
        seg: Segment,
    ) -> Result<Target<'a>, HclError> {
        let matching_count = node
            .blocks()
            .filter(|b| {
                b.ident.value().as_str() == ident
                    && labels_match(
                        &b.labels.iter().map(|l| l.as_str()).collect::<Vec<_>>(),
                        labels,
                    )
            })
            .count();

        if matching_count == 0 {
            return Err(self.not_found(seg));
        }

        if matching_count > 1 && nth.is_none() {
            return Err(HclError::AmbiguousBlock {
                ident: ident.to_string(),
                labels: labels.to_vec(),
                count: matching_count,
            });
        }

        let target_index = nth.unwrap_or(0);

        let block = node
            .blocks_mut()
            .filter(|b| {
                b.ident.value().as_str() == ident
                    && labels_match(
                        &b.labels.iter().map(|l| l.as_str()).collect::<Vec<_>>(),
                        labels,
                    )
            })
            .nth(target_index)
            .ok_or_else(|| self.not_found(seg))?;

        self.visit_block(block)
    }

    fn visit_block<'a>(&mut self, node: &'a mut Block) -> Result<Target<'a>, HclError> {
        if self.done() {
            return Ok(Target::Block(node));
        }
        self.visit_body(&mut node.body)
    }

    fn visit_object<'a>(
        &mut self,
        node: &'a mut hcl::edit::expr::Object,
    ) -> Result<Target<'a>, HclError> {
        let Some(seg) = self.next() else {
            return Ok(Target::Object(node));
        };
        self.visited += 1;

        let key = match &seg {
            Segment::Text { text } => text.as_str(),
            Segment::Attr { key } => key.as_str(),
            Segment::Index { .. } => {
                return Err(HclError::InvalidType {
                    expected: "Text segment",
                    actual: "Index segment",
                });
            }
            Segment::Block { .. } => {
                return Err(HclError::InvalidType {
                    expected: "Text segment",
                    actual: "Block segment",
                });
            }
        };

        let obj_key = ObjectKey::from(Ident::new(key));
        if let Some(value) = node.get_mut(&obj_key) {
            return self.visit_expr(value.expr_mut());
        }

        Err(self.not_found(seg))
    }

    fn visit_array<'a>(
        &mut self,
        node: &'a mut hcl::edit::expr::Array,
    ) -> Result<Target<'a>, HclError> {
        let Some(seg) = self.next() else {
            return Ok(Target::Array(node));
        };
        self.visited += 1;

        let index = match &seg {
            Segment::Index { index } => *index,
            Segment::Text { .. } => {
                return Err(HclError::InvalidType {
                    expected: "Index segment",
                    actual: "Text segment",
                });
            }
            Segment::Block { .. } => {
                return Err(HclError::InvalidType {
                    expected: "Index segment",
                    actual: "Block segment",
                });
            }
            Segment::Attr { .. } => {
                return Err(HclError::InvalidType {
                    expected: "Index segment",
                    actual: "Attr segment",
                });
            }
        };

        if let Some(expr) = node.get_mut(index) {
            return self.visit_expr(expr);
        }

        Err(self.not_found(seg))
    }

    fn visit_expr<'a>(&mut self, node: &'a mut Expression) -> Result<Target<'a>, HclError> {
        if node.is_object() {
            let obj = node.as_object_mut().unwrap();
            return self.visit_object(obj);
        }

        if node.is_array() {
            let arr = node.as_array_mut().unwrap();
            return self.visit_array(arr);
        }

        if let Some(seg) = self.next() {
            return Err(self.not_found(seg));
        }

        Ok(Target::Expr(node))
    }
}
