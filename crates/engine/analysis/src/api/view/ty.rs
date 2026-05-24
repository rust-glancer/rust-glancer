//! Composite projection from declarations and paths into Body IR types.
//!
//! `BodyTy` is the common type vocabulary analysis exposes today, even when the source fact lives
//! in Semantic IR or DefMap. This view keeps the storage-specific projection rules out of query
//! orchestration.

use rg_body_ir::{
    BodyDeclarationRef, BodyLocalNominalTy, BodyNominalTy, BodyPrimitiveTy, BodyRef, BodyTy,
    BodyTypePathResolution, ResolvedEnumVariantRef, ScopeId,
};
use rg_def_map::Path;
use rg_semantic_ir::{
    FieldRef, SemanticDeclarationRef, SemanticItemRef, SemanticTypePathResolution, TypePathContext,
};

use crate::api::{Analysis, view::declaration::DeclarationRef};

pub(crate) struct TyView<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> TyView<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn ty_for_declaration(
        &self,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Option<BodyTy>> {
        match declaration {
            DeclarationRef::Module(_) => Ok(None),
            DeclarationRef::LocalDef(local_def) => {
                let Some(SemanticItemRef::TypeDef(ty)) = self
                    .analysis
                    .semantic_ir
                    .semantic_item_for_local_def(local_def)?
                else {
                    return Ok(None);
                };
                Ok(Some(BodyTy::Nominal(vec![BodyNominalTy::bare(ty)])))
            }
            DeclarationRef::Semantic(SemanticDeclarationRef::Item(SemanticItemRef::TypeDef(
                ty,
            ))) => Ok(Some(BodyTy::Nominal(vec![BodyNominalTy::bare(ty)]))),
            DeclarationRef::Semantic(SemanticDeclarationRef::Item(
                SemanticItemRef::Trait(_)
                | SemanticItemRef::Impl(_)
                | SemanticItemRef::Function(_)
                | SemanticItemRef::TypeAlias(_)
                | SemanticItemRef::Const(_)
                | SemanticItemRef::Static(_),
            )) => Ok(None),
            DeclarationRef::Semantic(SemanticDeclarationRef::Field(field)) => {
                self.ty_for_field(field)
            }
            DeclarationRef::Semantic(SemanticDeclarationRef::EnumVariant(variant)) => {
                self.ty_for_enum_variant(ResolvedEnumVariantRef::Semantic(variant))
            }
            DeclarationRef::Body(BodyDeclarationRef::Binding(binding)) => Ok(self
                .analysis
                .body_ir
                .body_data(binding.body)?
                .and_then(|body_data| body_data.binding(binding.binding))
                .map(|data| data.ty.clone())),
            DeclarationRef::Body(BodyDeclarationRef::Item(item)) => Ok(Some(BodyTy::LocalNominal(
                vec![BodyLocalNominalTy::bare(item)],
            ))),
            DeclarationRef::Body(BodyDeclarationRef::ValueItem(item)) => Ok(self
                .analysis
                .body_ir
                .body_data(item.body)?
                .and_then(|body_data| body_data.local_value_item(item.item))
                .and_then(|data| data.ty().cloned())
                .map(BodyTy::Syntax)),
            DeclarationRef::Body(BodyDeclarationRef::EnumVariant(variant)) => {
                self.ty_for_enum_variant(ResolvedEnumVariantRef::BodyLocal(variant))
            }
            DeclarationRef::Body(
                BodyDeclarationRef::Impl(_)
                | BodyDeclarationRef::Field(_)
                | BodyDeclarationRef::Function(_),
            ) => Ok(None),
        }
    }

    pub(crate) fn ty_for_type_path(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> anyhow::Result<BodyTy> {
        let resolution =
            self.analysis
                .semantic_ir
                .resolve_type_path(&self.analysis.def_map, context, path)?;
        if matches!(resolution, SemanticTypePathResolution::Unknown)
            && let Some(primitive) = path.single_name().and_then(BodyPrimitiveTy::from_name)
        {
            return Ok(BodyTy::Primitive(primitive));
        }

        Ok(Self::semantic_type_path_resolution_to_ty(resolution))
    }

    pub(crate) fn ty_for_body_type_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<BodyTy> {
        Ok(Self::body_type_path_resolution_to_ty(
            self.analysis.body_ir.resolve_type_path_in_scope(
                &self.analysis.def_map,
                &self.analysis.semantic_ir,
                body_ref,
                scope,
                path,
            )?,
        ))
    }

    fn ty_for_field(&self, field: FieldRef) -> anyhow::Result<Option<BodyTy>> {
        Ok(self.analysis.body_ir.ty_for_field(
            &self.analysis.def_map,
            &self.analysis.semantic_ir,
            field,
        )?)
    }

    fn ty_for_enum_variant(
        &self,
        variant: ResolvedEnumVariantRef,
    ) -> anyhow::Result<Option<BodyTy>> {
        match variant {
            ResolvedEnumVariantRef::Semantic(variant) => {
                let Some(data) = self.analysis.semantic_ir.enum_variant_data(variant)? else {
                    return Ok(None);
                };
                Ok(Some(BodyTy::Nominal(vec![BodyNominalTy::bare(data.owner)])))
            }
            ResolvedEnumVariantRef::BodyLocal(variant) => {
                Ok(Some(BodyTy::LocalNominal(vec![BodyLocalNominalTy::bare(
                    variant.item,
                )])))
            }
        }
    }

    fn semantic_type_path_resolution_to_ty(resolution: SemanticTypePathResolution) -> BodyTy {
        match resolution {
            SemanticTypePathResolution::SelfType(types) => {
                BodyTy::SelfTy(types.into_iter().map(BodyNominalTy::bare).collect())
            }
            SemanticTypePathResolution::TypeDefs(types) => {
                BodyTy::Nominal(types.into_iter().map(BodyNominalTy::bare).collect())
            }
            // Traits are navigable symbols, but they are not value-like receiver types in this
            // small analysis model.
            SemanticTypePathResolution::Traits(_) => BodyTy::Unknown,
            SemanticTypePathResolution::Unknown => BodyTy::Unknown,
        }
    }

    fn body_type_path_resolution_to_ty(resolution: BodyTypePathResolution) -> BodyTy {
        match resolution {
            BodyTypePathResolution::BodyLocal(item) => {
                BodyTy::LocalNominal(vec![BodyLocalNominalTy::bare(item)])
            }
            BodyTypePathResolution::SelfType(types) => {
                BodyTy::SelfTy(types.into_iter().map(BodyNominalTy::bare).collect())
            }
            BodyTypePathResolution::TypeDefs(types) => {
                BodyTy::Nominal(types.into_iter().map(BodyNominalTy::bare).collect())
            }
            BodyTypePathResolution::Primitive(primitive) => BodyTy::Primitive(primitive),
            // Trait paths are useful for goto-definition, but type queries report only nominal
            // values and body-local item types.
            BodyTypePathResolution::Traits(_) => BodyTy::Unknown,
            BodyTypePathResolution::Unknown => BodyTy::Unknown,
        }
    }
}
