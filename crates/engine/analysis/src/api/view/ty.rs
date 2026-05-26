//! Composite projection from declarations and paths into Body IR types.
//!
//! `BodyTy` is the common type vocabulary analysis exposes today, even when the source fact lives
//! in Semantic IR or DefMap. This view keeps the storage-specific projection rules out of query
//! orchestration.

use rg_body_ir::{
    BodyAutoderef, BodyLocalNominalTy, BodyNominalTy, BodyRef, BodyTy, BodyTyExt, BodyTyRepr,
    BodyTypePathResolution, ScopeId,
};
use rg_def_map::Path;
use rg_semantic_ir::{
    FieldRef as SemanticFieldRef, SemanticItemRef, SemanticTypePathResolution, TypePathContext,
};

use crate::{
    api::{Analysis, resolve::declaration::SymbolDeclarationResolver},
    model::{
        DeclarationRef, DeclarationRefRepr, EnumVariantRef, EnumVariantRefRepr, FieldRefRepr,
        ItemRefRepr, NameDefRefRepr, SymbolAt, TypePathScopeRepr,
    },
};

pub(crate) struct TyView<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> TyView<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn ty_for_symbol(&self, symbol: SymbolAt) -> anyhow::Result<Option<BodyTy>> {
        let ty = match symbol {
            SymbolAt::Expr { expr } => self
                .analysis
                .body_ir
                .body_data(expr.body_ir())?
                .and_then(|body_data| body_data.expr(expr.expr_id()))
                .map(|data| data.ty.clone()),
            declaration_symbol @ SymbolAt::Declaration { .. } => {
                let declarations = SymbolDeclarationResolver::new(self.analysis)
                    .declarations_for_symbol(declaration_symbol)?;
                let mut ty = None;
                for declaration in declarations {
                    if let Some(declaration_ty) = self.ty_for_declaration(declaration)? {
                        ty = Some(declaration_ty);
                        break;
                    }
                }
                ty
            }
            SymbolAt::TypePath { scope, path, .. } => match scope.repr() {
                TypePathScopeRepr::Signature(context) => {
                    Some(self.ty_for_type_path(context, &path)?)
                }
                TypePathScopeRepr::Body(scope) => {
                    Some(self.ty_for_body_type_path(scope.body_ir(), scope.scope_id(), &path)?)
                }
            },
            SymbolAt::ValuePath { scope, path, .. } => {
                Some(self.ty_for_body_value_path(scope.body_ir(), scope.scope_id(), &path)?)
            }
            SymbolAt::UsePath { .. } => None,
            SymbolAt::FunctionBody { .. } => None,
        };
        Ok(ty)
    }

    pub(crate) fn declarations_for_ty(&self, ty: &BodyTy) -> Vec<DeclarationRef> {
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

    fn ty_for_declaration(&self, declaration: DeclarationRef) -> anyhow::Result<Option<BodyTy>> {
        match declaration.repr() {
            DeclarationRefRepr::Module(_) => Ok(None),
            DeclarationRefRepr::NameDef(name_def) => {
                let NameDefRefRepr::DefMapLocal(local_def) = name_def.repr();
                let Some(SemanticItemRef::TypeDef(ty)) = self
                    .analysis
                    .semantic_ir
                    .semantic_item_for_local_def(local_def)?
                else {
                    return Ok(None);
                };
                Ok(Some(BodyTyRepr::nominal(vec![BodyNominalTy::bare(ty)])))
            }
            DeclarationRefRepr::Item(item) => match item.repr() {
                ItemRefRepr::Semantic(SemanticItemRef::TypeDef(ty)) => {
                    Ok(Some(BodyTyRepr::nominal(vec![BodyNominalTy::bare(ty)])))
                }
                ItemRefRepr::Semantic(
                    SemanticItemRef::Trait(_)
                    | SemanticItemRef::Impl(_)
                    | SemanticItemRef::Function(_)
                    | SemanticItemRef::TypeAlias(_)
                    | SemanticItemRef::Const(_)
                    | SemanticItemRef::Static(_),
                ) => Ok(None),
                ItemRefRepr::BodyLocal(item) => Ok(Some(BodyTyRepr::local_nominal(vec![
                    BodyLocalNominalTy::bare(item),
                ]))),
                ItemRefRepr::BodyLocalValue(item) => Ok(self
                    .analysis
                    .body_ir
                    .body_data(item.body)?
                    .and_then(|body_data| body_data.local_value_item(item.item))
                    .and_then(|data| data.ty().cloned())
                    .map(BodyTyRepr::syntax)),
            },
            DeclarationRefRepr::Field(field) => match field.repr() {
                FieldRefRepr::Semantic(field) => self.ty_for_field(field),
                FieldRefRepr::BodyLocal(_) => Ok(None),
            },
            DeclarationRefRepr::EnumVariant(variant) => self.ty_for_enum_variant(variant),
            DeclarationRefRepr::Binding(binding) => {
                let binding = binding.body_ir();
                Ok(self
                    .analysis
                    .body_ir
                    .body_data(binding.body)?
                    .and_then(|body_data| body_data.binding(binding.binding))
                    .map(|data| data.ty.clone()))
            }
            DeclarationRefRepr::Function(_) | DeclarationRefRepr::Impl(_) => Ok(None),
        }
    }

    fn ty_for_type_path(&self, context: TypePathContext, path: &Path) -> anyhow::Result<BodyTy> {
        let resolution =
            self.analysis
                .semantic_ir
                .resolve_type_path(&self.analysis.def_map, context, path)?;
        if matches!(resolution, SemanticTypePathResolution::Unknown)
            && let Some(primitive) = path.single_name().and_then(rg_ty::PrimitiveTy::from_name)
        {
            return Ok(BodyTy::Primitive(primitive));
        }

        Ok(Self::semantic_type_path_resolution_to_ty(resolution))
    }

    fn ty_for_body_type_path(
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

    fn ty_for_body_value_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<BodyTy> {
        // Value-path type queries should use the same Body IR resolver as the main body pass, so
        // enum variants and associated functions agree between snapshots and cursor queries.
        let (_, ty) = self.analysis.body_ir.resolve_value_path_in_scope(
            &self.analysis.def_map,
            &self.analysis.semantic_ir,
            body_ref,
            scope,
            path,
        )?;
        Ok(ty)
    }

    fn ty_for_field(&self, field: SemanticFieldRef) -> anyhow::Result<Option<BodyTy>> {
        Ok(self.analysis.body_ir.ty_for_field(
            &self.analysis.def_map,
            &self.analysis.semantic_ir,
            field,
        )?)
    }

    fn ty_for_enum_variant(&self, variant: EnumVariantRef) -> anyhow::Result<Option<BodyTy>> {
        match variant.repr() {
            EnumVariantRefRepr::Semantic(variant) => {
                let Some(data) = self.analysis.semantic_ir.enum_variant_data(variant)? else {
                    return Ok(None);
                };
                Ok(Some(BodyTyRepr::nominal(vec![BodyNominalTy::bare(
                    data.owner,
                )])))
            }
            EnumVariantRefRepr::BodyLocal(variant) => Ok(Some(BodyTyRepr::local_nominal(vec![
                BodyLocalNominalTy::bare(variant.item),
            ]))),
        }
    }

    fn semantic_type_path_resolution_to_ty(resolution: SemanticTypePathResolution) -> BodyTy {
        match resolution {
            SemanticTypePathResolution::SelfType(types) => {
                BodyTyRepr::self_ty(types.into_iter().map(BodyNominalTy::bare).collect())
            }
            SemanticTypePathResolution::TypeDefs(types) => {
                BodyTyRepr::nominal(types.into_iter().map(BodyNominalTy::bare).collect())
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
                BodyTyRepr::local_nominal(vec![BodyLocalNominalTy::bare(item)])
            }
            BodyTypePathResolution::SelfType(types) => {
                BodyTyRepr::self_ty(types.into_iter().map(BodyNominalTy::bare).collect())
            }
            BodyTypePathResolution::TypeDefs(types) => {
                BodyTyRepr::nominal(types.into_iter().map(BodyNominalTy::bare).collect())
            }
            BodyTypePathResolution::Primitive(primitive) => BodyTy::Primitive(primitive),
            // Trait paths are useful for goto-definition, but type queries report only nominal
            // values and body-local item types.
            BodyTypePathResolution::Traits(_) => BodyTy::Unknown,
            BodyTypePathResolution::Unknown => BodyTy::Unknown,
        }
    }
}
