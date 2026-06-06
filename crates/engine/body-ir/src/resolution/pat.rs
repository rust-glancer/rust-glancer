//! Pattern-directed binding type propagation.
//!
//! This pass stays deliberately narrow: it only pushes already-known expected types into pattern
//! bindings. Enum variants are matched against a known enum scrutinee/annotation type; patterns do
//! not infer the scrutinee type by themselves.

use rg_ir_model::{BindingId, ExprId, PatId, ScopeId, StmtId, TypeDefId};
use rg_ir_storage::{
    DefMapSource, ItemStoreQuery, ItemStoreSource, Path, PathSegment, TargetItemQuery,
};
use rg_item_tree::{FieldItem, FieldKey, FieldList};
use rg_package_store::PackageStoreError;
use rg_ty::{
    ItemPathQuery, IterationItemResolver, NominalTy, ReferencePeelingCandidates, Ty, TypeSubst,
};

use crate::{
    ir::expr::ExprKind,
    ir::pat::{PatKind, RecordPatField},
    ir::path::BodyPath,
    ir::stmt::StmtKind,
};

use super::{BodyQuerySource, TypeRefUseSite, push_unique, type_path::BodyTypePathResolver};

pub(super) struct PatternTypePropagator<'query, D, I> {
    source: BodyQuerySource<'query, D, I>,
}

impl<'query, D, I> PatternTypePropagator<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(super) fn new(source: BodyQuerySource<'query, D, I>) -> Self {
        Self { source }
    }

    pub(super) fn propagate(&self) -> Result<Vec<(BindingId, Ty)>, PackageStoreError> {
        let mut updates = Vec::new();

        for statement_idx in 0..self.source.body().statements.len() {
            let statement = StmtId(statement_idx);
            let StmtKind::Let {
                scope,
                pat: Some(pat),
                annotation,
                initializer,
                ..
            } = self.source.body().statements[statement].kind.clone()
            else {
                continue;
            };

            let expected_ty = self.expected_ty_for_let(scope, annotation.as_ref(), initializer)?;
            self.propagate_pat(pat, &expected_ty, &mut updates)?;
        }

        for expr_idx in 0..self.source.body().exprs.len() {
            let expr = ExprId(expr_idx);
            match self.source.body().exprs[expr].kind.clone() {
                ExprKind::Match { scrutinee, arms } => {
                    let Some(scrutinee) = scrutinee else {
                        continue;
                    };
                    let expected_ty = self.source.body().exprs[scrutinee].ty.clone();
                    for arm in arms {
                        if let Some(pat) = arm.pat {
                            self.propagate_pat(pat, &expected_ty, &mut updates)?;
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
                    self.propagate_pat(pat, &expected_ty, &mut updates)?;
                }
                ExprKind::For {
                    pat: Some(pat),
                    iterable: Some(iterable),
                    ..
                } => {
                    let iterable_ty = &self.source.body().exprs[iterable].ty;
                    let item_ty = self
                        .iteration_items()
                        .into_iterator_item_for_ty(iterable_ty)?;
                    self.propagate_pat(pat, &item_ty, &mut updates)?;
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

        Ok(updates)
    }

    fn item_query(&self) -> ItemStoreQuery<'query, BodyQuerySource<'query, D, I>> {
        ItemStoreQuery::new(self.source)
    }

    fn type_path_resolver(&self) -> BodyTypePathResolver<'query, D, I> {
        BodyTypePathResolver::new(self.source)
    }

    fn iteration_items(
        &self,
    ) -> IterationItemResolver<'query, BodyQuerySource<'query, D, I>, BodyQuerySource<'query, D, I>>
    {
        let source = self.source;
        let item_paths = ItemPathQuery::new(source, source);
        let target_items = TargetItemQuery::new(source, source, self.source.body_ref().target);
        IterationItemResolver::new(item_paths, target_items)
    }

    fn expected_ty_for_let(
        &self,
        scope: ScopeId,
        annotation: Option<&rg_item_tree::TypeRef>,
        initializer: Option<ExprId>,
    ) -> Result<Ty, PackageStoreError> {
        if let Some(annotation) = annotation {
            let ty = self
                .type_path_resolver()
                .type_ref(TypeRefUseSite::Scope(scope))
                .resolve(annotation)?;
            if !matches!(ty, Ty::Unknown) {
                return Ok(ty);
            }
        }

        Ok(initializer
            .map(|expr| self.source.body().exprs[expr].ty.clone())
            .unwrap_or(Ty::Unknown))
    }

    fn propagate_pat(
        &self,
        pat: PatId,
        expected_ty: &Ty,
        updates: &mut Vec<(BindingId, Ty)>,
    ) -> Result<(), PackageStoreError> {
        if matches!(expected_ty, Ty::Unknown) {
            return Ok(());
        }

        let Some(data) = self.source.body().pat(pat).cloned() else {
            return Ok(());
        };

        match data.kind {
            PatKind::Binding {
                binding, subpat, ..
            } => {
                if let Some(binding) = binding {
                    self.push_binding_ty_update(binding, expected_ty.clone(), updates);
                }
                if let Some(subpat) = subpat {
                    self.propagate_pat(subpat, expected_ty, updates)?;
                }
                Ok(())
            }
            PatKind::TupleStruct { path, fields } => {
                self.propagate_tuple_variant(path.as_ref(), &fields, expected_ty, updates)
            }
            PatKind::Record { path, fields, .. } => {
                self.propagate_record_variant(path.as_ref(), &fields, expected_ty, updates)
            }
            PatKind::Tuple { fields } => self.propagate_tuple_pat(&fields, expected_ty, updates),
            PatKind::Slice { fields } => self.propagate_slice_pat(&fields, expected_ty, updates),
            PatKind::Or { pats } => {
                for pat in pats {
                    self.propagate_pat(pat, expected_ty, updates)?;
                }
                Ok(())
            }
            PatKind::Ref { pat, .. } | PatKind::Box { pat } => {
                self.propagate_pat(pat, expected_ty, updates)
            }
            PatKind::Path { .. }
            | PatKind::Rest
            | PatKind::Literal { .. }
            | PatKind::Range { .. }
            | PatKind::ConstBlock { .. }
            | PatKind::Wildcard
            | PatKind::Unsupported => Ok(()),
        }
    }

    fn propagate_tuple_pat(
        &self,
        fields: &[PatId],
        expected_ty: &Ty,
        updates: &mut Vec<(BindingId, Ty)>,
    ) -> Result<(), PackageStoreError> {
        let Ty::Tuple(field_tys) = expected_ty else {
            return Ok(());
        };
        if fields.len() != field_tys.len() {
            return Ok(());
        }

        for (field_pat, field_ty) in fields.iter().zip(field_tys) {
            self.propagate_pat(*field_pat, field_ty, updates)?;
        }
        Ok(())
    }

    fn propagate_slice_pat(
        &self,
        fields: &[PatId],
        expected_ty: &Ty,
        updates: &mut Vec<(BindingId, Ty)>,
    ) -> Result<(), PackageStoreError> {
        let element_ty = match expected_ty {
            Ty::Array { inner, .. } | Ty::Slice(inner) => inner.as_ref(),
            _ => return Ok(()),
        };

        for field in fields {
            if self
                .source
                .body()
                .pat(*field)
                .is_some_and(|pat| matches!(&pat.kind, PatKind::Rest))
            {
                continue;
            }
            self.propagate_pat(*field, element_ty, updates)?;
        }
        Ok(())
    }

    fn propagate_tuple_variant(
        &self,
        path: Option<&BodyPath>,
        fields: &[PatId],
        expected_ty: &Ty,
        updates: &mut Vec<(BindingId, Ty)>,
    ) -> Result<(), PackageStoreError> {
        let def_map_path = path.and_then(|path| path.as_def_map_path());
        let Some(variant_name) = variant_name(def_map_path.as_ref()) else {
            return Ok(());
        };

        for (idx, field_pat) in fields.iter().enumerate() {
            let field_key = FieldKey::Tuple(idx);
            if let Some(field_ty) = self.variant_field_ty(expected_ty, variant_name, &field_key)? {
                self.propagate_pat(*field_pat, &field_ty, updates)?;
            }
        }
        Ok(())
    }

    fn propagate_record_variant(
        &self,
        path: Option<&BodyPath>,
        fields: &[RecordPatField],
        expected_ty: &Ty,
        updates: &mut Vec<(BindingId, Ty)>,
    ) -> Result<(), PackageStoreError> {
        let def_map_path = path.and_then(|path| path.as_def_map_path());
        let Some(variant_name) = variant_name(def_map_path.as_ref()) else {
            return Ok(());
        };

        for field in fields {
            if let Some(field_ty) = self.variant_field_ty(expected_ty, variant_name, &field.key)? {
                self.propagate_pat(field.pat, &field_ty, updates)?;
            }
        }
        Ok(())
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
        for deref_candidate in ReferencePeelingCandidates::new(expected_ty) {
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
            .map(|generics| TypeSubst::from_generics(generics, &enum_ty.args))
            .unwrap_or_else(TypeSubst::new);

        Ok(Some(
            self.type_path_resolver()
                .type_ref(TypeRefUseSite::Module(variant_data.owner_module))
                .with_subst(&subst)
                .resolve(&field.ty)?,
        ))
    }

    fn push_binding_ty_update(
        &self,
        binding: BindingId,
        ty: Ty,
        updates: &mut Vec<(BindingId, Ty)>,
    ) {
        if matches!(ty, Ty::Unknown) {
            return;
        }

        let Some(binding_data) = self.source.body().bindings.get(binding) else {
            return;
        };
        if !matches!(binding_data.ty, Ty::Unknown) {
            return;
        }

        updates.push((binding, ty));
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
