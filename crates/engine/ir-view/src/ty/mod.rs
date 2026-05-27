//! Composite projection from declarations and paths into indexed types.
//!
//! `IndexedTy` is the common type vocabulary analysis exposes today, even when the source fact lives
//! in Semantic IR or DefMap. This view keeps the storage-specific projection rules out of query
//! orchestration.

pub mod implementation;
pub mod locals;
pub mod member;

use rg_body_ir::{BodyAutoderef, BodyTypePathResolution};
use rg_def_map::Path;
use rg_ir_model::{
    BodyRef, FieldRef as SemanticFieldRef, ScopeId, SemanticItemRef,
    identity::ExprRef,
    identity::{
        DeclarationRef, DeclarationRefRepr, EnumVariantRef, EnumVariantRefRepr, FieldRefRepr,
        ItemRefRepr, NameDefRefRepr,
    },
};
use rg_semantic_ir::{SemanticTypePathResolution, TypePathContext};
use rg_ty::{IndexedLocalNominalTy, IndexedNominalTy, IndexedTy, IndexedTyExt, IndexedTyRepr};

use crate::{IndexedViewDb, ty::locals::BodyView};

pub struct TyView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> TyView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub fn ty_for_expr(&self, expr: ExprRef) -> anyhow::Result<Option<IndexedTy>> {
        self.body_view().expr_ty(expr.body_ir(), expr.expr_id())
    }

    pub fn declarations_for_ty(&self, ty: &IndexedTy) -> Vec<DeclarationRef> {
        // Body-local nominal types shadow module-level types in the same expression type. Preserve
        // that lookup order when turning an inferred type back into navigation declarations.
        let mut local_declarations = Vec::new();
        for candidate in BodyAutoderef::peel_references(ty) {
            for ty in candidate.ty().as_local_nominals() {
                let declaration = DeclarationRef::body_item(ty.item);
                if !local_declarations.contains(&declaration) {
                    local_declarations.push(declaration);
                }
            }
        }
        if !local_declarations.is_empty() {
            return local_declarations;
        }

        let mut declarations = Vec::new();
        for candidate in BodyAutoderef::peel_references(ty) {
            for ty in candidate.ty().as_nominals() {
                let declaration = DeclarationRef::semantic(ty.def.into());
                if !declarations.contains(&declaration) {
                    declarations.push(declaration);
                }
            }
        }
        declarations
    }

    pub fn ty_for_declaration(
        &self,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Option<IndexedTy>> {
        match declaration.repr() {
            DeclarationRefRepr::Module(_) => Ok(None),
            DeclarationRefRepr::NameDef(name_def) => {
                let NameDefRefRepr::DefMapLocal(local_def) = name_def.repr();
                let Some(SemanticItemRef::TypeDef(ty)) =
                    self.db.semantic_ir.semantic_item_for_local_def(local_def)?
                else {
                    return Ok(None);
                };
                Ok(Some(IndexedTyRepr::nominal(vec![IndexedNominalTy::bare(
                    ty,
                )])))
            }
            DeclarationRefRepr::Item(item) => match item.repr() {
                ItemRefRepr::Semantic(SemanticItemRef::TypeDef(ty)) => Ok(Some(
                    IndexedTyRepr::nominal(vec![IndexedNominalTy::bare(ty)]),
                )),
                ItemRefRepr::Semantic(
                    SemanticItemRef::Trait(_)
                    | SemanticItemRef::Impl(_)
                    | SemanticItemRef::Function(_)
                    | SemanticItemRef::TypeAlias(_)
                    | SemanticItemRef::Const(_)
                    | SemanticItemRef::Static(_),
                ) => Ok(None),
                ItemRefRepr::BodyLocal(item) => Ok(Some(IndexedTyRepr::local_nominal(vec![
                    IndexedLocalNominalTy::bare(item),
                ]))),
                ItemRefRepr::BodyLocalValue(item) => self.body_view().local_value_item_ty(item),
            },
            DeclarationRefRepr::Field(field) => match field.repr() {
                FieldRefRepr::Semantic(field) => self.ty_for_field(field),
                FieldRefRepr::BodyLocal(_) => Ok(None),
            },
            DeclarationRefRepr::EnumVariant(variant) => self.ty_for_enum_variant(variant),
            DeclarationRefRepr::Binding(binding) => {
                let binding = binding.body_ir();
                self.body_view().binding_ty(binding)
            }
            DeclarationRefRepr::Function(_) | DeclarationRefRepr::Impl(_) => Ok(None),
        }
    }

    pub fn ty_for_type_path(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> anyhow::Result<IndexedTy> {
        let resolution = self
            .db
            .semantic_ir
            .resolve_type_path(&self.db.def_map, context, path)?;
        if matches!(resolution, SemanticTypePathResolution::Unknown)
            && let Some(primitive) = path.single_name().and_then(rg_ty::PrimitiveTy::from_name)
        {
            return Ok(IndexedTy::Primitive(primitive));
        }

        Ok(Self::semantic_type_path_resolution_to_ty(resolution))
    }

    pub fn ty_for_body_type_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<IndexedTy> {
        Ok(Self::body_type_path_resolution_to_ty(
            self.db.body_ir.resolve_type_path_in_scope(
                &self.db.def_map,
                &self.db.semantic_ir,
                body_ref,
                scope,
                path,
            )?,
        ))
    }

    pub fn ty_for_body_value_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<IndexedTy> {
        // Value-path type queries should use the same Body IR resolver as the main body pass, so
        // enum variants and associated functions agree between snapshots and cursor queries.
        let (_, ty) = self.db.body_ir.resolve_value_path_in_scope(
            &self.db.def_map,
            &self.db.semantic_ir,
            body_ref,
            scope,
            path,
        )?;
        Ok(ty)
    }

    fn ty_for_field(&self, field: SemanticFieldRef) -> anyhow::Result<Option<IndexedTy>> {
        Ok(self
            .db
            .body_ir
            .ty_for_field(&self.db.def_map, &self.db.semantic_ir, field)?)
    }

    fn ty_for_enum_variant(&self, variant: EnumVariantRef) -> anyhow::Result<Option<IndexedTy>> {
        match variant.repr() {
            EnumVariantRefRepr::Semantic(variant) => {
                let Some(data) = self.db.semantic_ir.enum_variant_data(variant)? else {
                    return Ok(None);
                };
                Ok(Some(IndexedTyRepr::nominal(vec![IndexedNominalTy::bare(
                    data.owner,
                )])))
            }
            EnumVariantRefRepr::BodyLocal(variant) => Ok(Some(IndexedTyRepr::local_nominal(vec![
                IndexedLocalNominalTy::bare(variant.item),
            ]))),
        }
    }

    fn semantic_type_path_resolution_to_ty(resolution: SemanticTypePathResolution) -> IndexedTy {
        match resolution {
            SemanticTypePathResolution::SelfType(types) => {
                IndexedTyRepr::self_ty(types.into_iter().map(IndexedNominalTy::bare).collect())
            }
            SemanticTypePathResolution::TypeDefs(types) => {
                IndexedTyRepr::nominal(types.into_iter().map(IndexedNominalTy::bare).collect())
            }
            // Traits are navigable symbols, but they are not value-like receiver types in this
            // small analysis model.
            SemanticTypePathResolution::Traits(_) => IndexedTy::Unknown,
            SemanticTypePathResolution::Unknown => IndexedTy::Unknown,
        }
    }

    fn body_type_path_resolution_to_ty(resolution: BodyTypePathResolution) -> IndexedTy {
        match resolution {
            BodyTypePathResolution::BodyLocal(item) => {
                IndexedTyRepr::local_nominal(vec![IndexedLocalNominalTy::bare(item)])
            }
            BodyTypePathResolution::SelfType(types) => {
                IndexedTyRepr::self_ty(types.into_iter().map(IndexedNominalTy::bare).collect())
            }
            BodyTypePathResolution::TypeDefs(types) => {
                IndexedTyRepr::nominal(types.into_iter().map(IndexedNominalTy::bare).collect())
            }
            BodyTypePathResolution::Primitive(primitive) => IndexedTy::Primitive(primitive),
            // Trait paths are useful for goto-definition, but type queries report only nominal
            // values and body-local item types.
            BodyTypePathResolution::Traits(_) => IndexedTy::Unknown,
            BodyTypePathResolution::Unknown => IndexedTy::Unknown,
        }
    }

    fn body_view(&self) -> BodyView<'a, 'db> {
        BodyView::new(self.db)
    }
}
