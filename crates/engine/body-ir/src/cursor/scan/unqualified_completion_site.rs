//! Unqualified completion site scanning.
//!
//! This scanner recognizes single-segment paths such as `inp$0` and `Us$0`.
//! Qualified paths are left to the path-completion scanner because their
//! candidate set comes from the resolved qualifier rather than lexical scope.

use rg_def_map::TargetRef;
use rg_item_tree::{GenericArg, TypeBound, TypePath, TypeRef};
use rg_package_store::PackageStoreError;
use rg_parse::FileId;

use crate::{
    BodyData, BodyId, BodyIrReadTxn, BodyPath, BodyRef, ExprKind, ScopeId, StmtKind,
    cursor::{UnqualifiedCompletionNamespace, UnqualifiedCompletionSite},
};

/// Finds the source site that belongs to an unqualified completion offset.
pub(crate) struct UnqualifiedCompletionSiteScanner<'txn, 'db> {
    body_ir: &'txn BodyIrReadTxn<'db>,
    target: TargetRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> UnqualifiedCompletionSiteScanner<'txn, 'db> {
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

    /// Returns the smallest type or value name prefix that accepts completions.
    pub(crate) fn site_at_name(
        &self,
    ) -> Result<Option<UnqualifiedCompletionSite>, PackageStoreError> {
        let Some(target_bodies) = self.body_ir.target_bodies(self.target)? else {
            return Ok(None);
        };
        let mut best: Option<(UnqualifiedCompletionSite, u32)> = None;

        for (body_idx, body) in target_bodies.bodies().iter().enumerate() {
            if body.source.file_id != self.file_id || !body.source.span.contains(self.offset) {
                continue;
            }

            let body_ref = BodyRef {
                target: self.target,
                body: BodyId(body_idx),
            };
            self.scan_type_names(body_ref, body, &mut best);
            self.scan_value_names(body_ref, body, &mut best);
        }

        Ok(best.map(|(site, _)| site))
    }

    /// Scans body-local type annotations, including nested generic arguments.
    fn scan_type_names(
        &self,
        body_ref: BodyRef,
        body: &BodyData,
        best: &mut Option<(UnqualifiedCompletionSite, u32)>,
    ) {
        for statement in body.statements.iter() {
            if statement.source.file_id != self.file_id {
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
            self.scan_type_ref(body_ref, *scope, body.bindings().len(), annotation, best);
        }

        for expr in body.exprs.iter() {
            if expr.source.file_id != self.file_id {
                continue;
            }
            let ExprKind::Closure {
                scope,
                params,
                ret_ty,
                ..
            } = &expr.kind
            else {
                continue;
            };
            for param in params {
                if let Some(annotation) = &param.annotation {
                    self.scan_type_ref(body_ref, *scope, body.bindings().len(), annotation, best);
                }
            }
            if let Some(ret_ty) = ret_ty {
                self.scan_type_ref(body_ref, *scope, body.bindings().len(), ret_ty, best);
            }
        }
    }

    fn scan_type_ref(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        visible_bindings: usize,
        ty: &TypeRef,
        best: &mut Option<(UnqualifiedCompletionSite, u32)>,
    ) {
        match ty {
            TypeRef::Path(path) => {
                if let Some(site) = self.site_for_type_path(body_ref, scope, visible_bindings, path)
                {
                    Self::remember_site(site, path.source_span.len(), best);
                }

                for segment in &path.segments {
                    for arg in &segment.args {
                        self.scan_generic_arg(body_ref, scope, visible_bindings, arg, best);
                    }
                }
            }
            TypeRef::Tuple(types) => {
                for ty in types {
                    self.scan_type_ref(body_ref, scope, visible_bindings, ty, best);
                }
            }
            TypeRef::Reference { inner, .. }
            | TypeRef::RawPointer { inner, .. }
            | TypeRef::Slice(inner) => {
                self.scan_type_ref(body_ref, scope, visible_bindings, inner, best);
            }
            TypeRef::Array { inner, .. } => {
                self.scan_type_ref(body_ref, scope, visible_bindings, inner, best);
            }
            TypeRef::FnPointer { params, ret } => {
                for param in params {
                    self.scan_type_ref(body_ref, scope, visible_bindings, param, best);
                }
                self.scan_type_ref(body_ref, scope, visible_bindings, ret, best);
            }
            TypeRef::ImplTrait(bounds) | TypeRef::DynTrait(bounds) => {
                for bound in bounds {
                    if let TypeBound::Trait(ty) = bound {
                        self.scan_type_ref(body_ref, scope, visible_bindings, ty, best);
                    }
                }
            }
            TypeRef::Unknown(_) | TypeRef::Never | TypeRef::Unit | TypeRef::Infer => {}
        }
    }

    fn scan_generic_arg(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        visible_bindings: usize,
        arg: &GenericArg,
        best: &mut Option<(UnqualifiedCompletionSite, u32)>,
    ) {
        match arg {
            GenericArg::Type(ty) => {
                self.scan_type_ref(body_ref, scope, visible_bindings, ty, best);
            }
            GenericArg::AssocType { ty: Some(ty), .. } => {
                self.scan_type_ref(body_ref, scope, visible_bindings, ty, best);
            }
            GenericArg::Lifetime(_)
            | GenericArg::Const(_)
            | GenericArg::AssocType { ty: None, .. }
            | GenericArg::Unsupported(_) => {}
        }
    }

    /// Scans expression paths where value-namespace completions can appear.
    fn scan_value_names(
        &self,
        body_ref: BodyRef,
        body: &BodyData,
        best: &mut Option<(UnqualifiedCompletionSite, u32)>,
    ) {
        for (_expr, expr_data) in body.exprs.iter_with_ids() {
            if expr_data.source.file_id != self.file_id {
                continue;
            }
            if let ExprKind::Path { path } = &expr_data.kind {
                self.scan_body_path(
                    body_ref,
                    expr_data.scope,
                    expr_data.visible_bindings,
                    path,
                    best,
                );
            }
        }
    }

    fn site_for_type_path(
        &self,
        body: BodyRef,
        scope: ScopeId,
        visible_bindings: usize,
        path: &TypePath,
    ) -> Option<UnqualifiedCompletionSite> {
        if path.absolute || path.segments.len() != 1 {
            return None;
        }
        let segment = path.segments.first()?;
        if !segment.span.touches(self.offset) {
            return None;
        }

        Some(UnqualifiedCompletionSite {
            body,
            scope,
            member_prefix_span: segment.span,
            namespace: UnqualifiedCompletionNamespace::Types,
            visible_bindings,
        })
    }

    fn scan_body_path(
        &self,
        body: BodyRef,
        scope: ScopeId,
        visible_bindings: usize,
        path: &BodyPath,
        best: &mut Option<(UnqualifiedCompletionSite, u32)>,
    ) {
        if path.path.absolute || path.segment_count() != 1 {
            return;
        }
        let Some(span) = path.segment_span(0) else {
            return;
        };
        if !span.touches(self.offset) {
            return;
        }

        Self::remember_site(
            UnqualifiedCompletionSite {
                body,
                scope,
                member_prefix_span: span,
                namespace: UnqualifiedCompletionNamespace::Values,
                visible_bindings,
            },
            path.source_span.len(),
            best,
        );
    }

    /// Keeps nested path behavior predictable by choosing the shortest matching path syntax.
    fn remember_site(
        site: UnqualifiedCompletionSite,
        source_len: u32,
        best: &mut Option<(UnqualifiedCompletionSite, u32)>,
    ) {
        if best
            .as_ref()
            .is_none_or(|(_, best_len)| source_len < *best_len)
        {
            *best = Some((site, source_len));
        }
    }
}
