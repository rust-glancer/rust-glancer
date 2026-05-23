//! Source-level declaration lookup shared by editor queries.

use rg_body_ir::{
    BodyImplId, BodyItemRef, BodyRef, BodyValueItemRef, ResolvedEnumVariantRef, ResolvedFieldRef,
    ResolvedFunctionRef,
};
use rg_def_map::{LocalDefRef, ModuleOrigin, ModuleRef};
use rg_semantic_ir::{
    ConstRef, FunctionRef, ImplRef, SemanticItemKind, SemanticItemRef, StaticRef, TraitRef,
    TypeAliasRef, TypeDefRef, TypeRef,
};

use crate::{
    api::{Analysis, view::member::MemberLookup},
    model::{Declaration, SymbolKind},
};

/// Storage-independent identity for declarations that editor features can project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::From)]
pub(crate) enum DeclarationRef {
    Module(ModuleRef),
    LocalDef(LocalDefRef),
    SemanticItem(SemanticItemRef),
    BodyImpl(BodyImplRef),
    BodyItem(BodyItemRef),
    BodyValueItem(BodyValueItemRef),
    EnumVariant(ResolvedEnumVariantRef),
    Field(ResolvedFieldRef),
    Function(ResolvedFunctionRef),
}

impl From<TypeDefRef> for DeclarationRef {
    fn from(item: TypeDefRef) -> Self {
        Self::SemanticItem(item.into())
    }
}

impl From<TraitRef> for DeclarationRef {
    fn from(item: TraitRef) -> Self {
        Self::SemanticItem(item.into())
    }
}

impl From<ImplRef> for DeclarationRef {
    fn from(item: ImplRef) -> Self {
        Self::SemanticItem(item.into())
    }
}

impl From<FunctionRef> for DeclarationRef {
    fn from(item: FunctionRef) -> Self {
        Self::SemanticItem(item.into())
    }
}

impl From<TypeAliasRef> for DeclarationRef {
    fn from(item: TypeAliasRef) -> Self {
        Self::SemanticItem(item.into())
    }
}

impl From<ConstRef> for DeclarationRef {
    fn from(item: ConstRef) -> Self {
        Self::SemanticItem(item.into())
    }
}

impl From<StaticRef> for DeclarationRef {
    fn from(item: StaticRef) -> Self {
        Self::SemanticItem(item.into())
    }
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
            DeclarationRef::SemanticItem(item) => self.semantic_item(item),
            DeclarationRef::BodyImpl(impl_ref) => self.body_impl(impl_ref),
            DeclarationRef::BodyItem(item_ref) => self.body_item(item_ref),
            DeclarationRef::BodyValueItem(item_ref) => self.body_value_item(item_ref),
            DeclarationRef::EnumVariant(variant) => self.enum_variant(variant),
            DeclarationRef::Field(field) => self.field(field),
            DeclarationRef::Function(function) => self.function(function),
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

    fn semantic_item(&self, item: SemanticItemRef) -> anyhow::Result<Option<Declaration>> {
        let Some(view) = self.analysis.semantic_ir.semantic_item_view(item)? else {
            return Ok(None);
        };

        match view.kind() {
            SemanticItemKind::Struct
            | SemanticItemKind::Enum
            | SemanticItemKind::Union
            | SemanticItemKind::Trait => {
                let Some(local_def) = view.local_def() else {
                    return Ok(None);
                };
                self.local_def(local_def)
            }
            SemanticItemKind::Impl => {
                let Some(local_impl_ref) = view.local_impl() else {
                    return Ok(None);
                };
                let Some(local_impl) = self.analysis.def_map.local_impl(local_impl_ref)? else {
                    return Ok(None);
                };
                let Some((self_ty, trait_ref)) = view.impl_header() else {
                    return Ok(None);
                };

                Ok(Some(Declaration {
                    target: item.target(),
                    kind: SymbolKind::Impl,
                    name: Self::impl_label(self_ty, trait_ref),
                    file_id: local_impl.file_id,
                    span: local_impl.span,
                    selection_span: local_impl.span,
                }))
            }
            SemanticItemKind::Function => match item {
                SemanticItemRef::Function(function) => {
                    self.function(ResolvedFunctionRef::Semantic(function))
                }
                SemanticItemRef::TypeDef(_)
                | SemanticItemRef::Trait(_)
                | SemanticItemRef::Impl(_)
                | SemanticItemRef::TypeAlias(_)
                | SemanticItemRef::Const(_)
                | SemanticItemRef::Static(_) => Ok(None),
            },
            SemanticItemKind::TypeAlias | SemanticItemKind::Const | SemanticItemKind::Static => {
                let Some(name) = view.name() else {
                    return Ok(None);
                };
                let Some(span) = view.span() else {
                    return Ok(None);
                };

                Ok(Some(Declaration {
                    target: item.target(),
                    kind: SymbolKind::from_semantic_item_kind(view.kind()),
                    name: name.to_string(),
                    file_id: view.source().file_id,
                    span,
                    selection_span: view.name_span().unwrap_or(span),
                }))
            }
        }
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

    fn impl_label(self_ty: &TypeRef, trait_ref: Option<&TypeRef>) -> String {
        match trait_ref {
            Some(trait_ref) => format!("impl {trait_ref} for {self_ty}"),
            None => format!("impl {self_ty}"),
        }
    }
}
