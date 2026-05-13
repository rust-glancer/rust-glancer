//! Private scanners that translate body source spans into cursor candidates.

use rg_def_map::{Path, TargetRef};
use rg_item_tree::{GenericArg, TypeBound, TypePath, TypeRef};
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span};

use crate::{
    BindingId, BodyData, BodyFieldRef, BodyFunctionId, BodyFunctionRef, BodyId, BodyIrReadTxn,
    BodyItemId, BodyItemRef, BodyPath, BodyRef, ExprData, ExprId, ExprKind, PatId, PatKind,
    ScopeId, StmtKind,
};

use super::{BodyCursorCandidate, DotReceiver};

/// Scans one Body IR transaction for all cursor candidates at a source offset.
pub(super) struct BodyCursorScanner<'txn, 'db> {
    body_ir: &'txn BodyIrReadTxn<'db>,
    target: TargetRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> BodyCursorScanner<'txn, 'db> {
    pub(super) fn new(
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

    pub(super) fn scan(&self) -> Result<Vec<BodyCursorCandidate>, PackageStoreError> {
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

/// Scans one target for every body-local source candidate used by whole-project queries.
pub(super) struct BodySourceScanner<'txn, 'db> {
    body_ir: &'txn BodyIrReadTxn<'db>,
    target: TargetRef,
    file_id: Option<FileId>,
}

impl<'txn, 'db> BodySourceScanner<'txn, 'db> {
    pub(super) fn new(
        body_ir: &'txn BodyIrReadTxn<'db>,
        target: TargetRef,
        file_id: Option<FileId>,
    ) -> Self {
        Self {
            body_ir,
            target,
            file_id,
        }
    }

    pub(super) fn scan(&self) -> Result<Vec<BodyCursorCandidate>, PackageStoreError> {
        let Some(target_bodies) = self.body_ir.target_bodies(self.target)? else {
            return Ok(Vec::new());
        };

        let mut candidates = Vec::new();
        for (body_idx, body) in target_bodies.bodies().iter().enumerate() {
            if !self.file_matches(body.source.file_id) {
                continue;
            }

            let body_ref = BodyRef {
                target: self.target,
                body: BodyId(body_idx),
            };

            self.push_declaration_candidates(body_ref, body, &mut candidates);
            self.push_member_reference_candidates(body_ref, body, &mut candidates);

            TypePathCursorScanner {
                body_ref,
                body,
                file_id: self.file_id,
                offset: None,
                candidates: &mut candidates,
            }
            .scan();
            ValuePathCursorScanner {
                body_ref,
                body,
                file_id: self.file_id,
                offset: None,
                include_single_segment: true,
                candidates: &mut candidates,
            }
            .scan();
        }

        Ok(candidates)
    }

    fn push_declaration_candidates(
        &self,
        body_ref: BodyRef,
        body: &BodyData,
        candidates: &mut Vec<BodyCursorCandidate>,
    ) {
        for (binding_idx, binding) in body.bindings().iter().enumerate() {
            if !self.file_matches(binding.source.file_id) {
                continue;
            }
            candidates.push(BodyCursorCandidate::Binding {
                body: body_ref,
                binding: BindingId(binding_idx),
                span: binding.source.span,
            });
        }

        for (item_idx, item) in body.local_items().iter().enumerate() {
            if !self.file_matches(item.name_source.file_id) {
                continue;
            }

            let item_ref = BodyItemRef {
                body: body_ref,
                item: BodyItemId(item_idx),
            };
            candidates.push(BodyCursorCandidate::LocalItem {
                item: item_ref,
                span: item.name_source.span,
            });

            for (field_idx, field) in item.fields.fields().iter().enumerate() {
                candidates.push(BodyCursorCandidate::LocalField {
                    field: BodyFieldRef {
                        item: item_ref,
                        index: field_idx,
                    },
                    span: field.span,
                });
            }
        }

        for (function_idx, function) in body.local_functions().iter().enumerate() {
            if !self.file_matches(function.name_source.file_id) {
                continue;
            }
            candidates.push(BodyCursorCandidate::LocalFunction {
                function: BodyFunctionRef {
                    body: body_ref,
                    function: BodyFunctionId(function_idx),
                },
                span: function.name_source.span,
            });
        }
    }

    fn push_member_reference_candidates(
        &self,
        body_ref: BodyRef,
        body: &BodyData,
        candidates: &mut Vec<BodyCursorCandidate>,
    ) {
        for (expr_idx, expr) in body.exprs().iter().enumerate() {
            if !self.file_matches(expr.source.file_id) {
                continue;
            }

            let span = match &expr.kind {
                ExprKind::Path { path } if path.segment_count() == 1 => {
                    path.segment_span(0).unwrap_or(expr.source.span)
                }
                ExprKind::MethodCall {
                    method_name_span: Some(span),
                    ..
                }
                | ExprKind::Field {
                    field_span: Some(span),
                    ..
                } => *span,
                ExprKind::MethodCall { .. } | ExprKind::Field { .. } => expr.source.span,
                _ => continue,
            };

            candidates.push(BodyCursorCandidate::Expr {
                body: body_ref,
                expr: ExprId(expr_idx),
                span,
            });
        }
    }

    fn file_matches(&self, file_id: FileId) -> bool {
        self.file_id.is_none_or(|selected| selected == file_id)
    }
}

/// Finds the receiver expression that belongs to a dot-completion offset.
pub(super) struct DotReceiverScanner<'txn, 'db> {
    body_ir: &'txn BodyIrReadTxn<'db>,
    target: TargetRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> DotReceiverScanner<'txn, 'db> {
    pub(super) fn new(
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

    pub(super) fn receiver_at_dot(&self) -> Result<Option<DotReceiver>, PackageStoreError> {
        let Some((body, receiver)) = self.receiver_expr_at_dot()? else {
            return Ok(None);
        };
        Ok(Some(DotReceiver { body, receiver }))
    }

    fn receiver_expr_at_dot(&self) -> Result<Option<(BodyRef, ExprId)>, PackageStoreError> {
        let Some(target_bodies) = self.body_ir.target_bodies(self.target)? else {
            return Ok(None);
        };
        let mut best = None::<(BodyRef, ExprId, u32)>;

        for (body_idx, body) in target_bodies.bodies().iter().enumerate() {
            if body.source.file_id != self.file_id || !body.source.span.contains(self.offset) {
                continue;
            }

            let body_ref = BodyRef {
                target: self.target,
                body: BodyId(body_idx),
            };
            for expr in body.exprs.iter() {
                if expr.source.file_id != self.file_id
                    || !Self::offset_in_dot_expr(expr, body, self.offset)
                {
                    continue;
                }

                let Some(receiver) = Self::receiver_expr(expr) else {
                    continue;
                };
                let len = expr.source.span.len();
                if best.is_none_or(|(_, _, best_len)| len < best_len) {
                    best = Some((body_ref, receiver, len));
                }
            }
        }

        Ok(best.map(|(body, receiver, _)| (body, receiver)))
    }

    fn offset_in_dot_expr(expr: &ExprData, body: &BodyData, offset: u32) -> bool {
        let Some(receiver) = Self::receiver_expr(expr) else {
            return false;
        };
        let Some(receiver_data) = body.expr(receiver) else {
            return false;
        };
        let Some(dot_span) = Self::dot_span(expr) else {
            return false;
        };
        let completion_end = Self::member_name_span(expr)
            .map(|span| span.text.end)
            .unwrap_or(expr.source.span.text.end);

        receiver_data.source.span.text.end <= dot_span.text.start
            && dot_span.text.end <= offset
            && offset <= completion_end
    }

    fn receiver_expr(expr: &ExprData) -> Option<ExprId> {
        match &expr.kind {
            ExprKind::MethodCall {
                receiver: Some(receiver),
                ..
            }
            | ExprKind::Field {
                base: Some(receiver),
                ..
            } => Some(*receiver),
            _ => None,
        }
    }

    fn member_name_span(expr: &ExprData) -> Option<Span> {
        match &expr.kind {
            ExprKind::MethodCall {
                method_name_span, ..
            } => *method_name_span,
            ExprKind::Field { field_span, .. } => *field_span,
            _ => None,
        }
    }

    fn dot_span(expr: &ExprData) -> Option<Span> {
        match &expr.kind {
            ExprKind::MethodCall { dot_span, .. } => *dot_span,
            ExprKind::Field { dot_span, .. } => *dot_span,
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceNodeAt {
    body: BodyRef,
    expr: Option<ExprId>,
    binding: Option<BindingId>,
    local_item: Option<BodyItemId>,
    local_field: Option<BodyFieldRef>,
    local_function: Option<BodyFunctionId>,
}

struct TypePathCursorScanner<'a> {
    body_ref: BodyRef,
    body: &'a BodyData,
    file_id: Option<FileId>,
    offset: Option<u32>,
    candidates: &'a mut Vec<BodyCursorCandidate>,
}

impl TypePathCursorScanner<'_> {
    fn scan(&mut self) {
        for statement in self.body.statements.iter() {
            if !self.file_matches(statement.source.file_id) {
                continue;
            }
            let StmtKind::Let {
                scope,
                annotation: Some(annotation),
                ..
            } = &statement.kind
            else {
                continue;
            };
            self.scan_type_ref(*scope, annotation, statement.source.file_id);
        }
    }

    fn scan_type_ref(&mut self, scope: ScopeId, ty: &TypeRef, file_id: FileId) {
        match ty {
            TypeRef::Path(path) => self.scan_type_path(scope, path, file_id),
            TypeRef::Tuple(types) => {
                for ty in types {
                    self.scan_type_ref(scope, ty, file_id);
                }
            }
            TypeRef::Reference { inner, .. }
            | TypeRef::RawPointer { inner, .. }
            | TypeRef::Slice(inner) => self.scan_type_ref(scope, inner, file_id),
            TypeRef::Array { inner, .. } => self.scan_type_ref(scope, inner, file_id),
            TypeRef::FnPointer { params, ret } => {
                for param in params {
                    self.scan_type_ref(scope, param, file_id);
                }
                self.scan_type_ref(scope, ret, file_id);
            }
            TypeRef::ImplTrait(bounds) | TypeRef::DynTrait(bounds) => {
                for bound in bounds {
                    if let TypeBound::Trait(ty) = bound {
                        self.scan_type_ref(scope, ty, file_id);
                    }
                }
            }
            TypeRef::Unknown(_) | TypeRef::Never | TypeRef::Unit | TypeRef::Infer => {}
        }
    }

    fn scan_type_path(&mut self, scope: ScopeId, path: &TypePath, file_id: FileId) {
        for (idx, segment) in path.segments.iter().enumerate() {
            if self.offset_matches(segment.span) {
                self.candidates.push(BodyCursorCandidate::TypePath {
                    body: self.body_ref,
                    scope,
                    path: Path::from_type_path_prefix(path, idx),
                    file_id,
                    span: segment.span,
                });
            }

            for arg in &segment.args {
                self.scan_generic_arg(scope, arg, file_id);
            }
        }
    }

    fn scan_generic_arg(&mut self, scope: ScopeId, arg: &GenericArg, file_id: FileId) {
        match arg {
            GenericArg::Type(ty) => self.scan_type_ref(scope, ty, file_id),
            GenericArg::AssocType { ty: Some(ty), .. } => {
                self.scan_type_ref(scope, ty, file_id);
            }
            GenericArg::Lifetime(_)
            | GenericArg::Const(_)
            | GenericArg::AssocType { ty: None, .. }
            | GenericArg::Unsupported(_) => {}
        }
    }

    fn file_matches(&self, file_id: FileId) -> bool {
        self.file_id.is_none_or(|selected| selected == file_id)
    }

    fn offset_matches(&self, span: Span) -> bool {
        self.offset.is_none_or(|offset| span.touches(offset))
    }
}

struct ValuePathCursorScanner<'a> {
    body_ref: BodyRef,
    body: &'a BodyData,
    file_id: Option<FileId>,
    offset: Option<u32>,
    include_single_segment: bool,
    candidates: &'a mut Vec<BodyCursorCandidate>,
}

impl ValuePathCursorScanner<'_> {
    fn scan(&mut self) {
        // Expression source-node lookup deliberately picks one smallest AST-ish node. Qualified
        // paths need finer granularity: in `Action::Start()`, `Action` and `Start` should produce
        // different symbols even though they belong to the same lowered expression.
        for (_expr, expr_data) in self.body.exprs.iter_with_ids() {
            if !self.file_matches(expr_data.source.file_id) {
                continue;
            }
            if let ExprKind::Path { path } = &expr_data.kind {
                self.scan_body_path(expr_data.scope, path, expr_data.source.file_id);
            }
        }

        // Pattern paths are not represented as expressions, but they are still editor-visible
        // value paths: `let Some(value) = option` and `Action::Start { .. }` should navigate from
        // both the enum name and the variant name.
        for statement in self.body.statements.iter() {
            if !self.file_matches(statement.source.file_id) {
                continue;
            }
            let StmtKind::Let {
                scope,
                pat: Some(pat),
                ..
            } = &statement.kind
            else {
                continue;
            };
            self.scan_pat(*scope, *pat);
        }

        for expr in self.body.exprs.iter() {
            if !self.file_matches(expr.source.file_id) {
                continue;
            }
            let ExprKind::Match { arms, .. } = &expr.kind else {
                continue;
            };
            for arm in arms {
                if let Some(pat) = arm.pat {
                    self.scan_pat(arm.scope, pat);
                }
            }
        }
    }

    fn scan_pat(&mut self, scope: ScopeId, pat: PatId) {
        let Some(data) = self.body.pat(pat) else {
            return;
        };

        match &data.kind {
            PatKind::TupleStruct { path, fields } => {
                if let Some(path) = path {
                    self.scan_body_path(scope, path, data.source.file_id);
                }
                for field in fields {
                    self.scan_pat(scope, *field);
                }
            }
            PatKind::Record { path, fields } => {
                if let Some(path) = path {
                    self.scan_body_path(scope, path, data.source.file_id);
                }
                for field in fields {
                    self.scan_pat(scope, field.pat);
                }
            }
            PatKind::Path { path } => {
                if let Some(path) = path {
                    self.scan_body_path(scope, path, data.source.file_id);
                }
            }
            PatKind::Binding { subpat, .. } => {
                if let Some(subpat) = subpat {
                    self.scan_pat(scope, *subpat);
                }
            }
            PatKind::Tuple { fields }
            | PatKind::Or { pats: fields }
            | PatKind::Slice { fields } => {
                for field in fields {
                    self.scan_pat(scope, *field);
                }
            }
            PatKind::Ref { pat } | PatKind::Box { pat } => self.scan_pat(scope, *pat),
            PatKind::Wildcard | PatKind::Unsupported => {}
        }
    }

    fn scan_body_path(&mut self, scope: ScopeId, path: &BodyPath, file_id: FileId) {
        // Single-segment expression paths are already represented by the surrounding expression
        // node. Segment candidates are only needed when the cursor can mean a prefix or an
        // associated item/variant inside one qualified path.
        if path.segment_count() <= 1 && !self.include_single_segment {
            return;
        }

        for idx in 0..path.segment_count() {
            let Some(span) = path.segment_span(idx) else {
                continue;
            };
            if self.offset_matches(span) {
                self.candidates.push(BodyCursorCandidate::ValuePath {
                    body: self.body_ref,
                    scope,
                    path: path.prefix_through(idx),
                    file_id,
                    span,
                });
            }
        }
    }

    fn file_matches(&self, file_id: FileId) -> bool {
        self.file_id.is_none_or(|selected| selected == file_id)
    }

    fn offset_matches(&self, span: Span) -> bool {
        self.offset.is_none_or(|offset| span.touches(offset))
    }
}
