//! Source-level declaration lookup shared by editor queries.

use rg_body_ir::{
    BodyImplId, BodyItemRef, BodyRef, BodyValueItemRef, ResolvedEnumVariantRef, ResolvedFieldRef,
    ResolvedFunctionRef,
};
use rg_def_map::{LocalDefRef, ModuleOrigin, ModuleRef};
use rg_semantic_ir::{ConstRef, ImplRef, StaticRef, TraitRef, TypeAliasRef, TypeDefRef, TypeRef};

use crate::{
    api::{Analysis, view::member::MemberLookup},
    model::{Declaration, SymbolKind},
};

/// Storage-independent identity for declarations that editor features can project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::From)]
pub(crate) enum DeclarationRef {
    Module(ModuleRef),
    LocalDef(LocalDefRef),
    TypeDef(TypeDefRef),
    Trait(TraitRef),
    Impl(ImplRef),
    BodyImpl(BodyImplRef),
    BodyItem(BodyItemRef),
    BodyValueItem(BodyValueItemRef),
    EnumVariant(ResolvedEnumVariantRef),
    Field(ResolvedFieldRef),
    Function(ResolvedFunctionRef),
    TypeAlias(TypeAliasRef),
    Const(ConstRef),
    Static(StaticRef),
}

/// Body IR stores impl ids inside a body, so the body id is part of the declaration identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BodyImplRef {
    pub(crate) body: BodyRef,
    pub(crate) impl_id: BodyImplId,
}

/// Reads declaration facts for IDs that already identify one source declaration.
pub(crate) struct DeclarationLookup<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> DeclarationLookup<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn declaration(
        &self,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Option<Declaration>> {
        match declaration {
            DeclarationRef::Module(module_ref) => self.module(module_ref),
            DeclarationRef::LocalDef(local_def) => self.local_def(local_def),
            DeclarationRef::TypeDef(ty) => self.type_def(ty),
            DeclarationRef::Trait(trait_ref) => self.trait_def(trait_ref),
            DeclarationRef::Impl(impl_ref) => self.impl_item(impl_ref),
            DeclarationRef::BodyImpl(impl_ref) => self.body_impl(impl_ref),
            DeclarationRef::BodyItem(item_ref) => self.body_item(item_ref),
            DeclarationRef::BodyValueItem(item_ref) => self.body_value_item(item_ref),
            DeclarationRef::EnumVariant(variant) => self.enum_variant(variant),
            DeclarationRef::Field(field) => self.field(field),
            DeclarationRef::Function(function) => self.function(function),
            DeclarationRef::TypeAlias(type_alias_ref) => self.type_alias(type_alias_ref),
            DeclarationRef::Const(const_ref) => self.const_item(const_ref),
            DeclarationRef::Static(static_ref) => self.static_item(static_ref),
        }
    }

    fn module(&self, module_ref: ModuleRef) -> anyhow::Result<Option<Declaration>> {
        let Some(module) = self.analysis.def_map.module(module_ref)? else {
            return Ok(None);
        };
        let Some(name) = module.name.as_ref().map(ToString::to_string) else {
            return Ok(None);
        };
        let (file_id, span) = match module.origin {
            ModuleOrigin::Root { .. } => return Ok(None),
            ModuleOrigin::Inline {
                declaration_file,
                declaration_span,
            }
            | ModuleOrigin::OutOfLine {
                declaration_file,
                declaration_span,
                ..
            } => (declaration_file, declaration_span),
        };

        Ok(Some(Declaration {
            target: module_ref.target,
            kind: SymbolKind::Module,
            name,
            file_id,
            span,
            selection_span: module.name_span.unwrap_or(span),
        }))
    }

    fn local_def(&self, local_def: LocalDefRef) -> anyhow::Result<Option<Declaration>> {
        let Some(data) = self.analysis.def_map.local_def(local_def)? else {
            return Ok(None);
        };

        Ok(Some(Declaration {
            target: local_def.target,
            kind: SymbolKind::from_local_def_kind(data.kind),
            name: data.name.to_string(),
            file_id: data.file_id,
            span: data.span,
            selection_span: data.name_span.unwrap_or(data.span),
        }))
    }

    fn type_def(&self, ty: TypeDefRef) -> anyhow::Result<Option<Declaration>> {
        let Some(local_def) = self.analysis.semantic_ir.local_def_for_type_def(ty)? else {
            return Ok(None);
        };

        self.local_def(local_def)
    }

    fn trait_def(&self, trait_ref: TraitRef) -> anyhow::Result<Option<Declaration>> {
        let Some(data) = self.analysis.semantic_ir.trait_data(trait_ref)? else {
            return Ok(None);
        };

        self.local_def(data.local_def)
    }

    fn impl_item(&self, impl_ref: ImplRef) -> anyhow::Result<Option<Declaration>> {
        let Some(data) = self.analysis.semantic_ir.impl_data(impl_ref)? else {
            return Ok(None);
        };
        let Some(local_impl) = self.analysis.def_map.local_impl(data.local_impl)? else {
            return Ok(None);
        };

        Ok(Some(Declaration {
            target: impl_ref.target,
            kind: SymbolKind::Impl,
            name: Self::impl_label(&data.self_ty, data.trait_ref.as_ref()),
            file_id: local_impl.file_id,
            span: local_impl.span,
            selection_span: local_impl.span,
        }))
    }

    fn body_impl(&self, impl_ref: BodyImplRef) -> anyhow::Result<Option<Declaration>> {
        let Some(body) = self.analysis.body_ir.body_data(impl_ref.body)? else {
            return Ok(None);
        };
        let Some(data) = body.local_impl(impl_ref.impl_id) else {
            return Ok(None);
        };

        Ok(Some(Declaration {
            target: impl_ref.body.target,
            kind: SymbolKind::Impl,
            name: Self::impl_label(&data.self_ty, data.trait_ref.as_ref()),
            file_id: data.source.file_id,
            span: data.source.span,
            selection_span: data.source.span,
        }))
    }

    fn body_item(&self, item_ref: BodyItemRef) -> anyhow::Result<Option<Declaration>> {
        let Some(body_data) = self.analysis.body_ir.body_data(item_ref.body)? else {
            return Ok(None);
        };
        let Some(item) = body_data.local_item(item_ref.item) else {
            return Ok(None);
        };

        Ok(Some(Declaration {
            target: item_ref.body.target,
            kind: SymbolKind::from_body_item_kind(item.kind),
            name: item.name.to_string(),
            file_id: item.source.file_id,
            span: item.source.span,
            selection_span: item.name_source.span,
        }))
    }

    fn body_value_item(&self, item_ref: BodyValueItemRef) -> anyhow::Result<Option<Declaration>> {
        let Some(body_data) = self.analysis.body_ir.body_data(item_ref.body)? else {
            return Ok(None);
        };
        let Some(item) = body_data.local_value_item(item_ref.item) else {
            return Ok(None);
        };

        Ok(Some(Declaration {
            target: item_ref.body.target,
            kind: SymbolKind::from_body_value_item_kind(item.kind),
            name: item.name.to_string(),
            file_id: item.source.file_id,
            span: item.source.span,
            selection_span: item.name_source.span,
        }))
    }

    fn enum_variant(&self, variant: ResolvedEnumVariantRef) -> anyhow::Result<Option<Declaration>> {
        match variant {
            ResolvedEnumVariantRef::Semantic(variant_ref) => {
                let Some(data) = self.analysis.semantic_ir.enum_variant_data(variant_ref)? else {
                    return Ok(None);
                };

                Ok(Some(Declaration {
                    target: variant_ref.target,
                    kind: SymbolKind::EnumVariant,
                    name: data.variant.name.to_string(),
                    file_id: data.file_id,
                    span: data.variant.span,
                    selection_span: data.variant.name_span,
                }))
            }
            ResolvedEnumVariantRef::BodyLocal(variant_ref) => {
                let Some(data) = self.analysis.body_ir.local_enum_variant_data(variant_ref)? else {
                    return Ok(None);
                };

                Ok(Some(Declaration {
                    target: variant_ref.item.body.target,
                    kind: SymbolKind::EnumVariant,
                    name: data.variant.name.to_string(),
                    file_id: data.item.source.file_id,
                    span: data.variant.span,
                    selection_span: data.variant.name_span,
                }))
            }
        }
    }

    fn field(&self, field: ResolvedFieldRef) -> anyhow::Result<Option<Declaration>> {
        Ok(MemberLookup::new(self.analysis)
            .field_view(field)?
            .and_then(|field| field.declaration()))
    }

    fn function(&self, function: ResolvedFunctionRef) -> anyhow::Result<Option<Declaration>> {
        Ok(MemberLookup::new(self.analysis)
            .function_view(function)?
            .map(|function| function.declaration()))
    }

    fn type_alias(&self, type_alias_ref: TypeAliasRef) -> anyhow::Result<Option<Declaration>> {
        let Some(data) = self.analysis.semantic_ir.type_alias_data(type_alias_ref)? else {
            return Ok(None);
        };

        Ok(Some(Declaration {
            target: type_alias_ref.target,
            kind: SymbolKind::TypeAlias,
            name: data.name.to_string(),
            file_id: data.source.file_id,
            span: data.span,
            selection_span: data.name_span.unwrap_or(data.span),
        }))
    }

    fn const_item(&self, const_ref: ConstRef) -> anyhow::Result<Option<Declaration>> {
        let Some(data) = self.analysis.semantic_ir.const_data(const_ref)? else {
            return Ok(None);
        };

        Ok(Some(Declaration {
            target: const_ref.target,
            kind: SymbolKind::Const,
            name: data.name.to_string(),
            file_id: data.source.file_id,
            span: data.span,
            selection_span: data.name_span.unwrap_or(data.span),
        }))
    }

    fn static_item(&self, static_ref: StaticRef) -> anyhow::Result<Option<Declaration>> {
        let Some(data) = self.analysis.semantic_ir.static_data(static_ref)? else {
            return Ok(None);
        };

        Ok(Some(Declaration {
            target: static_ref.target,
            kind: SymbolKind::Static,
            name: data.name.to_string(),
            file_id: data.source.file_id,
            span: data.span,
            selection_span: data.name_span.unwrap_or(data.span),
        }))
    }

    fn impl_label(self_ty: &TypeRef, trait_ref: Option<&TypeRef>) -> String {
        match trait_ref {
            Some(trait_ref) => format!("impl {trait_ref} for {self_ty}"),
            None => format!("impl {self_ty}"),
        }
    }
}
