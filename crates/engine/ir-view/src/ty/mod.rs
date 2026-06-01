//! Composite projection from declarations and paths into types.
//!
//! `Ty` is the common type vocabulary analysis exposes today. This view keeps projection rules
//! from Semantic IR, DefMap, and Body IR out of query orchestration.

pub mod implementation;
pub mod locals;
pub mod member;

use rg_body_ir::{BodyAutoderef, BodyTypePathResolution};
use rg_def_map::Path;
use rg_ir_model::{
    BodyRef, EnumVariantRef, FieldRef, ScopeId, SemanticItemRef, identity::DeclarationRef,
    identity::ExprRef,
};
use rg_semantic_ir::{SemanticTypePathResolution, TypePathContext};
use rg_ty::{NominalTy, Ty};

use crate::{IndexedViewDb, item::query::ItemQuery, ty::locals::BodyView};

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
        for candidate in BodyAutoderef::peel_references(ty) {
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
                    ItemQuery::new(self.db).semantic_item_for_local_def(local_def)?
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
        let resolution = self
            .db
            .semantic_ir
            .resolve_type_path(&self.db.def_map, context, path)?;
        if matches!(resolution, SemanticTypePathResolution::Unknown)
            && let Some(primitive) = path.single_name().and_then(rg_ty::PrimitiveTy::from_name)
        {
            return Ok(Ty::Primitive(primitive));
        }

        Ok(Self::semantic_type_path_resolution_to_ty(resolution))
    }

    pub fn ty_for_body_type_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Ty> {
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
    ) -> anyhow::Result<Ty> {
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

    fn ty_for_field(&self, field: FieldRef) -> anyhow::Result<Option<Ty>> {
        Ok(self
            .db
            .body_ir
            .ty_for_field(&self.db.def_map, &self.db.semantic_ir, field)?)
    }

    fn ty_for_enum_variant(&self, variant: EnumVariantRef) -> anyhow::Result<Option<Ty>> {
        let Some(data) = ItemQuery::new(self.db).enum_variant_data(variant)? else {
            return Ok(None);
        };
        Ok(Some(Ty::nominal(vec![NominalTy::bare(data.owner)])))
    }

    fn semantic_type_path_resolution_to_ty(resolution: SemanticTypePathResolution) -> Ty {
        match resolution {
            SemanticTypePathResolution::SelfType(types) => {
                Ty::self_ty(types.into_iter().map(NominalTy::bare).collect())
            }
            SemanticTypePathResolution::TypeDefs(types) => {
                Ty::nominal(types.into_iter().map(NominalTy::bare).collect())
            }
            // Traits are navigable symbols, but they are not value-like receiver types in this
            // small analysis model.
            SemanticTypePathResolution::Traits(_) => Ty::Unknown,
            SemanticTypePathResolution::Unknown => Ty::Unknown,
        }
    }

    fn body_type_path_resolution_to_ty(resolution: BodyTypePathResolution) -> Ty {
        match resolution {
            BodyTypePathResolution::SelfType(types) => {
                Ty::self_ty(types.into_iter().map(NominalTy::bare).collect())
            }
            BodyTypePathResolution::TypeDefs(types) => {
                Ty::nominal(types.into_iter().map(NominalTy::bare).collect())
            }
            BodyTypePathResolution::Primitive(primitive) => Ty::Primitive(primitive),
            // Trait paths are useful for goto-definition, but type queries report only nominal
            // values and body-local item types. Type aliases are expanded in Body IR before they
            // become expression or binding types.
            BodyTypePathResolution::TypeAliases(_) | BodyTypePathResolution::Traits(_) => {
                Ty::Unknown
            }
            BodyTypePathResolution::Unknown => Ty::Unknown,
        }
    }

    fn body_view(&self) -> BodyView<'a, 'db> {
        BodyView::new(self.db)
    }
}
