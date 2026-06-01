//! Pattern-directed binding type propagation.
//!
//! This pass stays deliberately narrow: it only pushes already-known expected types into pattern
//! bindings. Enum variants are matched against a known enum scrutinee/annotation type; patterns do
//! not infer the scrutinee type by themselves.

use rg_def_map::{DefMapReadTxn, Path, PathSegment};
use rg_ir_model::{BindingId, BodyRef, ExprId, PatId, ScopeId, StmtId, TypeDefId};
use rg_item_tree::{FieldItem, FieldKey, FieldList};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemStoreQuery, SemanticIrReadTxn};
use rg_ty::{NominalTy, Ty, TypeSubst};

use crate::{
    ir::body::BodyData,
    ir::expr::ExprKind,
    ir::pat::{PatKind, RecordPatField},
    ir::path::BodyPath,
    ir::stmt::StmtKind,
};

use super::{
    autoderef::BodyReferencePeelingCandidates, item_query::BodyItemStoreSource, push_unique,
    ty::subst_from_generics, type_path::BodyTypePathResolver,
};

pub(super) struct PatternTypePropagator<'query, 'db, 'body> {
    def_map: &'query DefMapReadTxn<'db>,
    semantic_ir: &'query SemanticIrReadTxn<'db>,
    body_ref: BodyRef,
    body: &'body mut BodyData,
}

impl<'query, 'db, 'body> PatternTypePropagator<'query, 'db, 'body> {
    pub(super) fn new(
        def_map: &'query DefMapReadTxn<'db>,
        semantic_ir: &'query SemanticIrReadTxn<'db>,
        body_ref: BodyRef,
        body: &'body mut BodyData,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            body_ref,
            body,
        }
    }

    pub(super) fn propagate(&mut self) -> Result<bool, PackageStoreError> {
        let mut changed = false;

        for statement_idx in 0..self.body.statements.len() {
            let statement = StmtId(statement_idx);
            let StmtKind::Let {
                scope,
                pat: Some(pat),
                annotation,
                initializer,
                ..
            } = self.body.statements[statement].kind.clone()
            else {
                continue;
            };

            let expected_ty = self.expected_ty_for_let(scope, annotation.as_ref(), initializer)?;
            changed |= self.propagate_pat(pat, &expected_ty)?;
        }

        for expr_idx in 0..self.body.exprs.len() {
            let expr = ExprId(expr_idx);
            match self.body.exprs[expr].kind.clone() {
                ExprKind::Match { scrutinee, arms } => {
                    let Some(scrutinee) = scrutinee else {
                        continue;
                    };
                    let expected_ty = self.body.exprs[scrutinee].ty.clone();
                    for arm in arms {
                        if let Some(pat) = arm.pat {
                            changed |= self.propagate_pat(pat, &expected_ty)?;
                        }
                    }
                }
                ExprKind::Let {
                    scope,
                    pat: Some(pat),
                    initializer,
                    ..
                } => {
                    let expected_ty = self.expected_ty_for_let(scope, None, initializer)?;
                    changed |= self.propagate_pat(pat, &expected_ty)?;
                }
                ExprKind::Path { .. }
                | ExprKind::Call { .. }
                | ExprKind::Tuple { .. }
                | ExprKind::Array { .. }
                | ExprKind::RepeatArray { .. }
                | ExprKind::Index { .. }
                | ExprKind::Range { .. }
                | ExprKind::Cast { .. }
                | ExprKind::Unary { .. }
                | ExprKind::Binary { .. }
                | ExprKind::Assign { .. }
                | ExprKind::If { .. }
                | ExprKind::Closure { .. }
                | ExprKind::Loop { .. }
                | ExprKind::While { .. }
                | ExprKind::For { .. }
                | ExprKind::Break { .. }
                | ExprKind::Continue { .. }
                | ExprKind::Block { .. }
                | ExprKind::Field { .. }
                | ExprKind::Record { .. }
                | ExprKind::MethodCall { .. }
                | ExprKind::Wrapper { .. }
                | ExprKind::Literal { .. }
                | ExprKind::Underscore
                | ExprKind::Yield { .. }
                | ExprKind::Yeet { .. }
                | ExprKind::Become { .. }
                | ExprKind::Let { pat: None, .. }
                | ExprKind::Unknown { .. } => {}
            }
        }

        Ok(changed)
    }

    fn item_query(&self) -> ItemStoreQuery<'_, BodyItemStoreSource<'_, 'db>> {
        ItemStoreQuery::new(BodyItemStoreSource::new(
            self.semantic_ir,
            self.body_ref,
            self.body,
        ))
    }

    fn expected_ty_for_let(
        &self,
        scope: ScopeId,
        annotation: Option<&rg_item_tree::TypeRef>,
        initializer: Option<ExprId>,
    ) -> Result<Ty, PackageStoreError> {
        if let Some(annotation) = annotation {
            let ty =
                BodyTypePathResolver::new(self.def_map, self.semantic_ir, self.body_ref, self.body)
                    .ty_from_type_ref_in_scope(annotation, scope)?;
            if !matches!(ty, Ty::Unknown) {
                return Ok(ty);
            }
        }

        Ok(initializer
            .map(|expr| self.body.exprs[expr].ty.clone())
            .unwrap_or(Ty::Unknown))
    }

    fn propagate_pat(&mut self, pat: PatId, expected_ty: &Ty) -> Result<bool, PackageStoreError> {
        if matches!(expected_ty, Ty::Unknown) {
            return Ok(false);
        }

        let Some(data) = self.body.pat(pat).cloned() else {
            return Ok(false);
        };

        match data.kind {
            PatKind::Binding {
                binding, subpat, ..
            } => {
                let mut changed = binding
                    .map(|binding| self.set_binding_ty(binding, expected_ty.clone()))
                    .unwrap_or(false);
                if let Some(subpat) = subpat {
                    changed |= self.propagate_pat(subpat, expected_ty)?;
                }
                Ok(changed)
            }
            PatKind::TupleStruct { path, fields } => {
                self.propagate_tuple_variant(path.as_ref(), &fields, expected_ty)
            }
            PatKind::Record { path, fields, .. } => {
                self.propagate_record_variant(path.as_ref(), &fields, expected_ty)
            }
            PatKind::Or { pats } => {
                let mut changed = false;
                for pat in pats {
                    changed |= self.propagate_pat(pat, expected_ty)?;
                }
                Ok(changed)
            }
            PatKind::Ref { pat, .. } | PatKind::Box { pat } => self.propagate_pat(pat, expected_ty),
            PatKind::Tuple { .. }
            | PatKind::Slice { .. }
            | PatKind::Path { .. }
            | PatKind::Rest
            | PatKind::Literal { .. }
            | PatKind::Range { .. }
            | PatKind::ConstBlock { .. }
            | PatKind::Wildcard
            | PatKind::Unsupported => Ok(false),
        }
    }

    fn propagate_tuple_variant(
        &mut self,
        path: Option<&BodyPath>,
        fields: &[PatId],
        expected_ty: &Ty,
    ) -> Result<bool, PackageStoreError> {
        let def_map_path = path.and_then(|path| path.as_def_map_path());
        let Some(variant_name) = variant_name(def_map_path.as_ref()) else {
            return Ok(false);
        };

        let mut changed = false;
        for (idx, field_pat) in fields.iter().enumerate() {
            let field_key = FieldKey::Tuple(idx);
            if let Some(field_ty) = self.variant_field_ty(expected_ty, variant_name, &field_key)? {
                changed |= self.propagate_pat(*field_pat, &field_ty)?;
            }
        }
        Ok(changed)
    }

    fn propagate_record_variant(
        &mut self,
        path: Option<&BodyPath>,
        fields: &[RecordPatField],
        expected_ty: &Ty,
    ) -> Result<bool, PackageStoreError> {
        let def_map_path = path.and_then(|path| path.as_def_map_path());
        let Some(variant_name) = variant_name(def_map_path.as_ref()) else {
            return Ok(false);
        };

        let mut changed = false;
        for field in fields {
            if let Some(field_ty) = self.variant_field_ty(expected_ty, variant_name, &field.key)? {
                changed |= self.propagate_pat(field.pat, &field_ty)?;
            }
        }
        Ok(changed)
    }

    fn variant_field_ty(
        &self,
        expected_ty: &Ty,
        variant_name: &str,
        field_key: &FieldKey,
    ) -> Result<Option<Ty>, PackageStoreError> {
        let mut candidates = Vec::new();

        // Pattern propagation peels only reference wrappers so enum payload inference remains
        // useful without opting into receiver autoderef or future trait-backed deref.
        for deref_candidate in BodyReferencePeelingCandidates::new(expected_ty) {
            for enum_ty in deref_candidate
                .ty()
                .as_nominals()
                .iter()
                .filter(|ty| matches!(ty.def.id, TypeDefId::Enum(_)))
            {
                let Some(field_ty) =
                    self.variant_field_ty_for_enum(enum_ty, variant_name, field_key)?
                else {
                    continue;
                };
                push_unique(&mut candidates, field_ty);
            }
        }

        match candidates.as_slice() {
            [ty] => Ok(Some(ty.clone())),
            [] | [_, ..] => Ok(None),
        }
    }

    fn variant_field_ty_for_enum(
        &self,
        enum_ty: &NominalTy,
        variant_name: &str,
        field_key: &FieldKey,
    ) -> Result<Option<Ty>, PackageStoreError> {
        if !matches!(enum_ty.def.id, TypeDefId::Enum(_)) {
            return Ok(None);
        }

        let Some(variant_ref) = self
            .item_query()
            .enum_variant_ref_for_type_def(enum_ty.def, variant_name)?
        else {
            return Ok(None);
        };
        let item_query = self.item_query();
        let Some(variant_data) = item_query.enum_variant_data(variant_ref)? else {
            return Ok(None);
        };
        let Some(field) = variant_field(&variant_data.variant.fields, field_key) else {
            return Ok(None);
        };
        let subst = item_query
            .generic_params_for_type_def(enum_ty.def)?
            .map(|generics| subst_from_generics(generics, &enum_ty.args))
            .unwrap_or_else(TypeSubst::new);

        Ok(Some(
            BodyTypePathResolver::new(self.def_map, self.semantic_ir, self.body_ref, self.body)
                .ty_from_type_ref_in_module_with_subst(
                    &field.ty,
                    variant_data.owner_module,
                    &subst,
                )?,
        ))
    }

    fn set_binding_ty(&mut self, binding: BindingId, ty: Ty) -> bool {
        if matches!(ty, Ty::Unknown) {
            return false;
        }

        let Some(binding_data) = self.body.bindings.get_mut(binding) else {
            return false;
        };
        if !matches!(binding_data.ty, Ty::Unknown) {
            return false;
        }

        binding_data.ty = ty;
        true
    }
}

fn variant_field<'a>(fields: &'a FieldList, key: &FieldKey) -> Option<&'a FieldItem> {
    match key {
        FieldKey::Named(_) => fields
            .fields()
            .iter()
            .find(|field| field.key.as_ref() == Some(key)),
        FieldKey::Tuple(index) => fields
            .fields()
            .get(*index)
            .filter(|field| field.key.as_ref() == Some(key)),
    }
}

fn variant_name(path: Option<&Path>) -> Option<&str> {
    match path?.segments.last()? {
        PathSegment::Name(name) => Some(name),
        PathSegment::SelfKw
        | PathSegment::SuperKw
        | PathSegment::CrateKw
        | PathSegment::DollarCrate(_) => None,
    }
}
