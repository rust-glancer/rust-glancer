//! Point-query scanning for editor requests at one source offset.
//!
//! Point queries pick the most specific body-local node under the cursor, then
//! add any extra path-segment candidates visible at the same offset.

use rg_def_map::TargetRef;
use rg_package_store::PackageStoreError;
use rg_parse::FileId;

use crate::{
    BindingId, BodyData, BodyFieldRef, BodyFunctionId, BodyFunctionRef, BodyId, BodyIrReadTxn,
    BodyItemId, BodyItemRef, BodyRef, ExprId,
};

use super::{
    super::BodyCursorCandidate,
    paths::{TypePathCursorScanner, ValuePathCursorScanner},
};

/// Scans one Body IR transaction for all cursor candidates at a source offset.
pub(crate) struct BodyCursorScanner<'txn, 'db> {
    body_ir: &'txn BodyIrReadTxn<'db>,
    target: TargetRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> BodyCursorScanner<'txn, 'db> {
    pub(crate) fn new(
        body_ir: &'txn BodyIrReadTxn<'db>,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> Self {
        Self {
            body_ir,
            target,
            file_id,
            offset,
        }
    }

    /// Returns body-local candidates that can answer an editor query at this exact offset.
    pub(crate) fn scan(&self) -> Result<Vec<BodyCursorCandidate>, PackageStoreError> {
        let Some(source_node) = self.source_node_at()? else {
            return Ok(Vec::new());
        };
        let Some(body) = self.body_ir.body_data(source_node.body)? else {
            return Ok(Vec::new());
        };

        let mut candidates = Vec::new();
        candidates.push(Self::candidate_for_source_node(body, source_node));
        TypePathCursorScanner {
            body_ref: source_node.body,
            body,
            file_id: Some(self.file_id),
            offset: Some(self.offset),
            candidates: &mut candidates,
        }
        .scan();
        ValuePathCursorScanner {
            body_ref: source_node.body,
            body,
            file_id: Some(self.file_id),
            offset: Some(self.offset),
            include_single_segment: false,
            candidates: &mut candidates,
        }
        .scan();

        Ok(candidates)
    }

    /// Finds the enclosing body and the smallest matching node in each body-local category.
    fn source_node_at(&self) -> Result<Option<SourceNodeAt>, PackageStoreError> {
        let Some(target_bodies) = self.body_ir.target_bodies(self.target)? else {
            return Ok(None);
        };
        let mut best = None;

        for (body_idx, body) in target_bodies.bodies().iter().enumerate() {
            if body.source.file_id != self.file_id || !body.source.span.contains(self.offset) {
                continue;
            }

            let body_ref = BodyRef {
                target: self.target,
                body: BodyId(body_idx),
            };
            best = Some(SourceNodeAt {
                body: body_ref,
                expr: Self::smallest_expr_at(body, self.file_id, self.offset),
                binding: Self::smallest_binding_at(body, self.file_id, self.offset),
                local_item: Self::smallest_local_item_at(body, self.file_id, self.offset),
                local_field: Self::smallest_local_field_at(
                    body_ref,
                    body,
                    self.file_id,
                    self.offset,
                ),
                local_function: Self::smallest_local_function_at(body, self.file_id, self.offset),
            });
        }

        Ok(best)
    }

    fn smallest_expr_at(body: &BodyData, file_id: FileId, offset: u32) -> Option<ExprId> {
        body.exprs
            .iter()
            .enumerate()
            .filter(|(_, expr)| expr.source.file_id == file_id)
            .filter(|(_, expr)| expr.source.span.touches(offset))
            .min_by_key(|(_, expr)| expr.source.span.len())
            .map(|(idx, _)| ExprId(idx))
    }

    fn smallest_binding_at(body: &BodyData, file_id: FileId, offset: u32) -> Option<BindingId> {
        body.bindings
            .iter()
            .enumerate()
            .filter(|(_, binding)| binding.source.file_id == file_id)
            .filter(|(_, binding)| binding.source.span.touches(offset))
            .min_by_key(|(_, binding)| binding.source.span.len())
            .map(|(idx, _)| BindingId(idx))
    }

    fn smallest_local_item_at(body: &BodyData, file_id: FileId, offset: u32) -> Option<BodyItemId> {
        body.local_items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.name_source.file_id == file_id)
            .filter(|(_, item)| item.name_source.span.touches(offset))
            .min_by_key(|(_, item)| item.name_source.span.len())
            .map(|(idx, _)| BodyItemId(idx))
    }

    fn smallest_local_field_at(
        body_ref: BodyRef,
        body: &BodyData,
        file_id: FileId,
        offset: u32,
    ) -> Option<BodyFieldRef> {
        body.local_items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.source.file_id == file_id)
            .flat_map(|(item_idx, item)| {
                item.fields
                    .fields()
                    .iter()
                    .enumerate()
                    .map(move |(field_idx, field)| (item_idx, field_idx, field))
            })
            .filter(|(_, _, field)| field.span.touches(offset))
            .min_by_key(|(_, _, field)| field.span.len())
            .map(|(item_idx, field_idx, _)| BodyFieldRef {
                item: BodyItemRef {
                    body: body_ref,
                    item: BodyItemId(item_idx),
                },
                index: field_idx,
            })
    }

    fn smallest_local_function_at(
        body: &BodyData,
        file_id: FileId,
        offset: u32,
    ) -> Option<BodyFunctionId> {
        body.local_functions
            .iter()
            .enumerate()
            .filter(|(_, function)| function.name_source.file_id == file_id)
            .filter(|(_, function)| function.name_source.span.touches(offset))
            .min_by_key(|(_, function)| function.name_source.span.len())
            .map(|(idx, _)| BodyFunctionId(idx))
    }

    /// Picks the most precise source node, falling back to the body itself when needed.
    fn candidate_for_source_node(
        body: &BodyData,
        source_node: SourceNodeAt,
    ) -> BodyCursorCandidate {
        let mut candidates = Vec::new();
        if let Some(expr) = source_node.expr
            && let Some(data) = body.expr(expr)
        {
            candidates.push((
                data.source.span.len(),
                BodyCursorCandidate::Expr {
                    body: source_node.body,
                    expr,
                    span: data.source.span,
                },
            ));
        }
        if let Some(binding) = source_node.binding
            && let Some(data) = body.binding(binding)
        {
            candidates.push((
                data.source.span.len(),
                BodyCursorCandidate::Binding {
                    body: source_node.body,
                    binding,
                    span: data.source.span,
                },
            ));
        }
        if let Some(item) = source_node.local_item
            && let Some(data) = body.local_item(item)
        {
            candidates.push((
                data.name_source.span.len(),
                BodyCursorCandidate::LocalItem {
                    item: BodyItemRef {
                        body: source_node.body,
                        item,
                    },
                    span: data.name_source.span,
                },
            ));
        }
        if let Some(field) = source_node.local_field
            && let Some(data) = body
                .local_item(field.item.item)
                .and_then(|item| item.field(field.index))
        {
            candidates.push((
                data.span.len(),
                BodyCursorCandidate::LocalField {
                    field,
                    span: data.span,
                },
            ));
        }
        if let Some(function) = source_node.local_function
            && let Some(data) = body.local_function(function)
        {
            candidates.push((
                data.name_source.span.len(),
                BodyCursorCandidate::LocalFunction {
                    function: BodyFunctionRef {
                        body: source_node.body,
                        function,
                    },
                    span: data.name_source.span,
                },
            ));
        }

        candidates
            .into_iter()
            .min_by_key(|(len, _)| *len)
            .map(|(_, candidate)| candidate)
            .unwrap_or(BodyCursorCandidate::Body {
                body: source_node.body,
                span: body.source.span,
            })
    }
}

/// Smallest body-local source node in each category at one cursor offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceNodeAt {
    body: BodyRef,
    expr: Option<ExprId>,
    binding: Option<BindingId>,
    local_item: Option<BodyItemId>,
    local_field: Option<BodyFieldRef>,
    local_function: Option<BodyFunctionId>,
}
