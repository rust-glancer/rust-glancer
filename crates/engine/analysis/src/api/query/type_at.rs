//! Best-effort type queries over analysis symbols.
//!
//! The public query returns Body IR types because they can describe both semantic items and
//! body-local declarations. Signature-only resolutions are converted into that common shape here.

use rg_body_ir::{
    BodyDeclarationRef, BodyLocalNominalTy, BodyNominalTy, BodyPrimitiveTy, BodyRef, BodyTy,
    BodyTypePathResolution, ResolvedEnumVariantRef, ScopeId,
};
use rg_def_map::Path;
use rg_semantic_ir::{
    FieldRef, SemanticDeclarationRef, SemanticItemRef, SemanticTypePathResolution, TypePathContext,
};

use crate::{
    api::{
        Analysis, resolve::declaration::SymbolDeclarationResolver,
        view::declaration::DeclarationRef,
    },
    model::SymbolAt,
};

pub(crate) struct TypeResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> TypeResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn type_at(
        &self,
        target: rg_def_map::TargetRef,
        file_id: rg_parse::FileId,
        offset: u32,
    ) -> anyhow::Result<Option<BodyTy>> {
        let Some(symbol) = self.0.symbol_at_for_query(target, file_id, offset)? else {
            return Ok(None);
        };

        let ty = match symbol {
            SymbolAt::Expr { body, expr } => self
                .0
                .body_ir
                .body_data(body)?
                .and_then(|body_data| body_data.expr(expr))
                .map(|data| data.ty.clone()),
            SymbolAt::Binding { body, binding } => self
                .0
                .body_ir
                .body_data(body)?
                .and_then(|body_data| body_data.binding(binding))
                .map(|data| data.ty.clone()),
            SymbolAt::BodyPath {
                body, scope, path, ..
            } => Some(self.ty_for_body_type_path(body, scope, &path)?),
            SymbolAt::BodyValuePath {
                body, scope, path, ..
            } => {
                // Value-path type queries should use the same Body IR resolver as the main body
                // pass, so enum variants and associated functions agree between snapshots and
                // cursor-driven editor queries.
                let (_, ty) = self.0.body_ir.resolve_value_path_in_scope(
                    &self.0.def_map,
                    &self.0.semantic_ir,
                    body,
                    scope,
                    &path,
                )?;
                Some(ty)
            }
            declaration_symbol @ (SymbolAt::Def { .. }
            | SymbolAt::Field { .. }
            | SymbolAt::Function { .. }
            | SymbolAt::EnumVariant { .. }
            | SymbolAt::LocalItem { .. }
            | SymbolAt::LocalValueItem { .. }
            | SymbolAt::LocalField { .. }
            | SymbolAt::LocalEnumVariant { .. }
            | SymbolAt::LocalFunction { .. }) => {
                self.ty_for_symbol_declarations(declaration_symbol)?
            }
            SymbolAt::TypePath { context, path, .. } => {
                Some(self.ty_for_type_path(context, &path)?)
            }
            SymbolAt::UsePath { .. } => None,
            SymbolAt::Body { .. } => None,
        };
        Ok(ty)
    }

    pub(crate) fn ty_for_type_path(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> anyhow::Result<BodyTy> {
        let resolution = self
            .0
            .semantic_ir
            .resolve_type_path(&self.0.def_map, context, path)?;
        if matches!(resolution, SemanticTypePathResolution::Unknown)
            && let Some(primitive) = path.single_name().and_then(BodyPrimitiveTy::from_name)
        {
            return Ok(BodyTy::Primitive(primitive));
        }

        Ok(semantic_type_path_resolution_to_ty(resolution))
    }

    pub(crate) fn ty_for_body_type_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<BodyTy> {
        Ok(body_type_path_resolution_to_ty(
            self.0.body_ir.resolve_type_path_in_scope(
                &self.0.def_map,
                &self.0.semantic_ir,
                body_ref,
                scope,
                path,
            )?,
        ))
    }

    fn ty_for_symbol_declarations(&self, symbol: SymbolAt) -> anyhow::Result<Option<BodyTy>> {
        let declarations =
            SymbolDeclarationResolver::new(self.0).declarations_for_symbol(symbol)?;
        for declaration in declarations {
            if let Some(ty) = self.ty_for_declaration(declaration)? {
                return Ok(Some(ty));
            }
        }
        Ok(None)
    }

    fn ty_for_declaration(&self, declaration: DeclarationRef) -> anyhow::Result<Option<BodyTy>> {
        match declaration {
            DeclarationRef::Module(_) => Ok(None),
            DeclarationRef::LocalDef(local_def) => {
                let Some(SemanticItemRef::TypeDef(ty)) =
                    self.0.semantic_ir.semantic_item_for_local_def(local_def)?
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
                .0
                .body_ir
                .body_data(binding.body)?
                .and_then(|body_data| body_data.binding(binding.binding))
                .map(|data| data.ty.clone())),
            DeclarationRef::Body(BodyDeclarationRef::Item(item)) => Ok(Some(BodyTy::LocalNominal(
                vec![BodyLocalNominalTy::bare(item)],
            ))),
            DeclarationRef::Body(BodyDeclarationRef::ValueItem(item)) => Ok(self
                .0
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

    fn ty_for_field(&self, field: FieldRef) -> anyhow::Result<Option<BodyTy>> {
        Ok(self
            .0
            .body_ir
            .ty_for_field(&self.0.def_map, &self.0.semantic_ir, field)?)
    }

    fn ty_for_enum_variant(
        &self,
        variant: ResolvedEnumVariantRef,
    ) -> anyhow::Result<Option<BodyTy>> {
        match variant {
            ResolvedEnumVariantRef::Semantic(variant) => {
                let Some(data) = self.0.semantic_ir.enum_variant_data(variant)? else {
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
}

pub(crate) fn semantic_type_path_resolution_to_ty(
    resolution: SemanticTypePathResolution,
) -> BodyTy {
    match resolution {
        SemanticTypePathResolution::SelfType(types) => {
            BodyTy::SelfTy(types.into_iter().map(BodyNominalTy::bare).collect())
        }
        SemanticTypePathResolution::TypeDefs(types) => {
            BodyTy::Nominal(types.into_iter().map(BodyNominalTy::bare).collect())
        }
        // Traits are navigable symbols, but they are not value-like receiver types in this small
        // analysis model.
        SemanticTypePathResolution::Traits(_) => BodyTy::Unknown,
        SemanticTypePathResolution::Unknown => BodyTy::Unknown,
    }
}

pub(crate) fn body_type_path_resolution_to_ty(resolution: BodyTypePathResolution) -> BodyTy {
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
        // Trait paths are useful for goto-definition, but `type_at` reports only nominal values
        // and body-local item types.
        BodyTypePathResolution::Traits(_) => BodyTy::Unknown,
        BodyTypePathResolution::Unknown => BodyTy::Unknown,
    }
}
