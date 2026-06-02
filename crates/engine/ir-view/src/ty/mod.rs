//! Composite projection from declarations and paths into types.
//!
//! `Ty` is the common type vocabulary analysis exposes today. This view keeps projection rules
//! from Semantic IR, DefMap, and Body IR out of query orchestration.

pub mod implementation;
pub mod locals;
pub mod member;

use rg_body_ir::BodyScopeQuery;
use rg_ir_model::{
    BodyRef, EnumVariantRef, FieldRef, ScopeId, SemanticItemRef, TypePathResolution,
    identity::DeclarationRef, identity::ExprRef,
};
use rg_ir_storage::{ItemStoreQuery, Path, TypePathContext};
use rg_ty::{ItemPathQuery, NominalTy, ReferencePeelingCandidates, Ty, TypeSubst};

use crate::{IndexedViewDb, ty::locals::BodyView};

pub struct TyView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> TyView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub fn ty_for_expr(&self, expr: ExprRef) -> anyhow::Result<Option<Ty>> {
        self.body_view().expr_ty(expr.body_ir(), expr.expr_id())
    }

    pub fn declarations_for_ty(&self, ty: &Ty) -> Vec<DeclarationRef> {
        let mut declarations = Vec::new();
        for candidate in ReferencePeelingCandidates::new(ty) {
            for ty in candidate.ty().as_nominals() {
                let declaration = DeclarationRef::from(ty.def);
                if !declarations.contains(&declaration) {
                    declarations.push(declaration);
                }
            }
        }
        declarations
    }

    pub fn ty_for_declaration(&self, declaration: DeclarationRef) -> anyhow::Result<Option<Ty>> {
        match declaration {
            DeclarationRef::Module(_) => Ok(None),
            DeclarationRef::LocalDef(local_def) => {
                let Some(SemanticItemRef::TypeDef(ty)) =
                    ItemStoreQuery::new(self.db).semantic_item_for_local_def(local_def)?
                else {
                    return Ok(None);
                };
                Ok(Some(Ty::nominal(vec![NominalTy::bare(ty)])))
            }
            DeclarationRef::Item(SemanticItemRef::TypeDef(ty)) => {
                Ok(Some(Ty::nominal(vec![NominalTy::bare(ty)])))
            }
            DeclarationRef::Item(
                SemanticItemRef::Trait(_)
                | SemanticItemRef::Impl(_)
                | SemanticItemRef::Function(_)
                | SemanticItemRef::TypeAlias(_)
                | SemanticItemRef::Const(_)
                | SemanticItemRef::Static(_),
            ) => Ok(None),
            DeclarationRef::Field(field) => self.ty_for_field(field),
            DeclarationRef::EnumVariant(variant) => self.ty_for_enum_variant(variant),
            DeclarationRef::BodyBinding(binding) => self.body_view().binding_ty(binding),
        }
    }

    pub fn ty_for_type_path(&self, context: TypePathContext, path: &Path) -> anyhow::Result<Ty> {
        let resolution = ItemPathQuery::new(self.db, self.db).resolve_type_path(context, path)?;
        if matches!(resolution, TypePathResolution::Unknown)
            && let Some(primitive) = path.single_name().and_then(rg_ty::PrimitiveTy::from_name)
        {
            return Ok(Ty::Primitive(primitive));
        }

        Ok(Self::type_path_resolution_to_ty(resolution))
    }

    pub fn ty_for_body_type_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Ty> {
        let Some(body) = self.db.body_ir.body_data(body_ref)? else {
            return Ok(Ty::Unknown);
        };
        let resolution = BodyScopeQuery::new(self.db, self.db, body_ref, body)
            .resolve_type_path_in_scope(scope, path)?;
        if matches!(resolution, TypePathResolution::Unknown)
            && let Some(primitive) = path.single_name().and_then(rg_ty::PrimitiveTy::from_name)
        {
            return Ok(Ty::Primitive(primitive));
        }

        Ok(Self::type_path_resolution_to_ty(resolution))
    }

    pub fn ty_for_body_value_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Ty> {
        // Value-path type queries should use the same Body IR resolver as the main body pass, so
        // enum variants and associated functions agree between snapshots and cursor queries.
        let Some(body) = self.db.body_ir.body_data(body_ref)? else {
            return Ok(Ty::Unknown);
        };
        let (_, ty) = BodyScopeQuery::new(self.db, self.db, body_ref, body)
            .resolve_value_path_in_scope(scope, path)?;
        Ok(ty)
    }

    fn ty_for_field(&self, field: FieldRef) -> anyhow::Result<Option<Ty>> {
        // Field declarations live in the shared item store, but view callers expect the small
        // `Ty` vocabulary used by body/member analysis.
        let Some(field_data) = ItemStoreQuery::new(self.db).field_data(field)? else {
            return Ok(None);
        };
        let item_paths = ItemPathQuery::new(self.db, self.db);
        Ok(Some(item_paths.resolve_type_ref(
            &field_data.field.ty,
            TypePathContext::module(field_data.owner_module),
            Ty::Unknown,
            &TypeSubst::new(),
        )?))
    }

    fn ty_for_enum_variant(&self, variant: EnumVariantRef) -> anyhow::Result<Option<Ty>> {
        let Some(data) = ItemStoreQuery::new(self.db).enum_variant_data(variant)? else {
            return Ok(None);
        };
        Ok(Some(Ty::nominal(vec![NominalTy::bare(data.owner)])))
    }

    fn type_path_resolution_to_ty(resolution: TypePathResolution) -> Ty {
        Ty::from_type_path_resolution(resolution, Vec::new()).unwrap_or(Ty::Unknown)
    }

    fn body_view(&self) -> BodyView<'a, 'db> {
        BodyView::new(self.db)
    }
}
