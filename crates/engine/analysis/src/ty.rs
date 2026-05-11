//! Best-effort type queries over analysis symbols.
//!
//! The public query returns Body IR types because they can describe both semantic items and
//! body-local declarations. Signature-only resolutions are converted into that common shape here.

use rg_body_ir::{
    BodyLocalNominalTy, BodyNominalTy, BodyRef, BodyTy, BodyTypePathResolution, ScopeId,
};
use rg_def_map::{DefId, Path};
use rg_semantic_ir::{
    FieldRef, ItemId, SemanticTypePathResolution, TypeDefId, TypeDefRef, TypePathContext,
};

use super::{Analysis, data::SymbolAt};

pub(super) struct TypeResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> TypeResolver<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(super) fn type_at(
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
            SymbolAt::Def { def, .. } => self.ty_for_def(def)?,
            SymbolAt::Field { field, .. } => self.ty_for_field(field)?,
            SymbolAt::LocalItem { item, .. } => {
                Some(BodyTy::LocalNominal(vec![BodyLocalNominalTy::bare(item)]))
            }
            SymbolAt::TypePath { context, path, .. } => {
                Some(self.ty_for_type_path(context, &path)?)
            }
            SymbolAt::EnumVariant { variant, .. } => self.ty_for_enum_variant(variant)?,
            SymbolAt::UsePath { .. } | SymbolAt::Function { .. } => None,
            SymbolAt::Body { .. } => None,
        };
        Ok(ty)
    }

    pub(super) fn ty_for_type_path(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> anyhow::Result<BodyTy> {
        Ok(semantic_type_path_resolution_to_ty(
            self.0
                .semantic_ir
                .resolve_type_path(&self.0.def_map, context, path)?,
        ))
    }

    pub(super) fn ty_for_body_type_path(
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

    fn ty_for_def(&self, def: DefId) -> anyhow::Result<Option<BodyTy>> {
        let DefId::Local(local_def) = def else {
            return Ok(None);
        };
        let Some(target_ir) = self.0.semantic_ir.target_ir(local_def.target)? else {
            return Ok(None);
        };
        let Some(item) = target_ir.item_for_local_def(local_def.local_def) else {
            return Ok(None);
        };
        let id = match item {
            ItemId::Struct(id) => TypeDefId::Struct(id),
            ItemId::Enum(id) => TypeDefId::Enum(id),
            ItemId::Union(id) => TypeDefId::Union(id),
            ItemId::Trait(_)
            | ItemId::Function(_)
            | ItemId::TypeAlias(_)
            | ItemId::Const(_)
            | ItemId::Static(_) => return Ok(None),
        };

        Ok(Some(BodyTy::Nominal(vec![BodyNominalTy::bare(
            TypeDefRef {
                target: local_def.target,
                id,
            },
        )])))
    }

    fn ty_for_field(&self, field: FieldRef) -> anyhow::Result<Option<BodyTy>> {
        Ok(self
            .0
            .body_ir
            .ty_for_field(&self.0.def_map, &self.0.semantic_ir, field)?)
    }

    fn ty_for_enum_variant(
        &self,
        variant: rg_semantic_ir::EnumVariantRef,
    ) -> anyhow::Result<Option<BodyTy>> {
        let Some(data) = self.0.semantic_ir.enum_variant_data(variant)? else {
            return Ok(None);
        };
        Ok(Some(BodyTy::Nominal(vec![BodyNominalTy::bare(data.owner)])))
    }
}

pub(super) fn semantic_type_path_resolution_to_ty(
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

pub(super) fn body_type_path_resolution_to_ty(resolution: BodyTypePathResolution) -> BodyTy {
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
        // Trait paths are useful for goto-definition, but `type_at` reports only nominal values
        // and body-local item types.
        BodyTypePathResolution::Traits(_) => BodyTy::Unknown,
        BodyTypePathResolution::Unknown => BodyTy::Unknown,
    }
}
