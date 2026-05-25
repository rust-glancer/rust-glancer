//! Composite declaration details used by editor features.
//!
//! Declarations identify source facts, but UI features usually need the surrounding presentation
//! facts as well: docs, display path, symbol kind, and a compact signature. This view keeps that
//! storage-specific projection out of feature queries.

use rg_body_ir::{
    BodyDeclarationRef, ResolvedEnumVariantRef, ResolvedFieldRef, ResolvedFunctionRef,
};
use rg_def_map::{LocalDefRef, ModuleRef};
use rg_semantic_ir::{
    ConstRef, Documentation, SemanticDeclarationRef, SemanticItemRef, StaticRef, TraitRef,
    TypeAliasRef, TypeDefId, TypeDefRef,
};

use crate::{
    api::{
        Analysis,
        render::{path::PathRenderer, signature::SignatureRenderer},
    },
    model::SymbolKind,
};

use super::{
    declaration::{DeclarationRef, DeclarationView},
    member::MemberView,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DeclarationDetailsContext {
    pub(crate) module_display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeclarationDetails {
    pub(crate) kind: SymbolKind,
    pub(crate) path: Option<String>,
    pub(crate) signature: Option<String>,
    pub(crate) docs: Option<String>,
}

pub(crate) struct DeclarationDetailsView<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> DeclarationDetailsView<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn details_for_declaration(
        &self,
        declaration: DeclarationRef,
        context: &DeclarationDetailsContext,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        match declaration {
            DeclarationRef::Module(module) => self.module_details(module, context),
            DeclarationRef::LocalDef(local_def) => self.local_def_details(local_def),
            DeclarationRef::Semantic(declaration) => self.semantic_declaration_details(declaration),
            DeclarationRef::Body(declaration) => self.body_declaration_details(declaration),
        }
    }

    fn body_declaration_details(
        &self,
        declaration: BodyDeclarationRef,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        match declaration {
            BodyDeclarationRef::Binding(_) => {
                let Some(view) = self.analysis.body_ir.body_declaration_view(declaration)? else {
                    return Ok(None);
                };
                let Some(binding_data) = view.binding_data() else {
                    return Ok(None);
                };
                Ok(Some(DeclarationDetails {
                    kind: SymbolKind::Variable,
                    path: None,
                    signature: Some(
                        SignatureRenderer::new(self.analysis).binding_signature(binding_data)?,
                    ),
                    docs: None,
                }))
            }
            BodyDeclarationRef::Item(_) => {
                let Some(view) = self.analysis.body_ir.body_declaration_view(declaration)? else {
                    return Ok(None);
                };
                let Some(item) = view.item_data() else {
                    return Ok(None);
                };
                let Some(declaration_view) =
                    DeclarationView::new(self.analysis).declaration(declaration.into())?
                else {
                    return Ok(None);
                };
                Ok(Some(DeclarationDetails {
                    kind: declaration_view.kind(),
                    path: None,
                    signature: Some(
                        SignatureRenderer::new(self.analysis).local_item_signature(item),
                    ),
                    docs: item.docs.as_ref().map(Documentation::text),
                }))
            }
            BodyDeclarationRef::ValueItem(_) => {
                let Some(view) = self.analysis.body_ir.body_declaration_view(declaration)? else {
                    return Ok(None);
                };
                let Some(item) = view.value_item_data() else {
                    return Ok(None);
                };
                let Some(declaration_view) =
                    DeclarationView::new(self.analysis).declaration(declaration.into())?
                else {
                    return Ok(None);
                };
                Ok(Some(DeclarationDetails {
                    kind: declaration_view.kind(),
                    path: None,
                    signature: Some(
                        SignatureRenderer::new(self.analysis).local_value_item_signature(item),
                    ),
                    docs: item.docs.as_ref().map(Documentation::text),
                }))
            }
            BodyDeclarationRef::Function(function) => {
                self.function_details(ResolvedFunctionRef::BodyLocal(function))
            }
            BodyDeclarationRef::Field(field) => {
                self.field_details(ResolvedFieldRef::BodyLocal(field))
            }
            BodyDeclarationRef::EnumVariant(variant) => {
                self.enum_variant_details(ResolvedEnumVariantRef::BodyLocal(variant))
            }
            BodyDeclarationRef::Impl(_) => Ok(None),
        }
    }

    fn semantic_declaration_details(
        &self,
        declaration: SemanticDeclarationRef,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        match declaration {
            SemanticDeclarationRef::Item(item) => self.semantic_item_details(item),
            SemanticDeclarationRef::Field(field) => {
                self.field_details(ResolvedFieldRef::Semantic(field))
            }
            SemanticDeclarationRef::EnumVariant(variant) => {
                self.enum_variant_details(ResolvedEnumVariantRef::Semantic(variant))
            }
        }
    }

    fn semantic_item_details(
        &self,
        item: SemanticItemRef,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        match item {
            SemanticItemRef::TypeDef(ty) => self.type_def_details(ty),
            SemanticItemRef::Trait(trait_ref) => self.trait_details(trait_ref),
            SemanticItemRef::Impl(_) => Ok(None),
            SemanticItemRef::Function(function) => {
                self.function_details(ResolvedFunctionRef::Semantic(function))
            }
            SemanticItemRef::TypeAlias(type_alias_ref) => self.type_alias_details(type_alias_ref),
            SemanticItemRef::Const(const_ref) => self.const_details(const_ref),
            SemanticItemRef::Static(static_ref) => self.static_details(static_ref),
        }
    }

    fn type_def_details(&self, ty: TypeDefRef) -> anyhow::Result<Option<DeclarationDetails>> {
        let Some(target_ir) = self.analysis.semantic_ir.target_ir(ty.target)? else {
            return Ok(None);
        };
        let renderer = SignatureRenderer::new(self.analysis);
        let path = PathRenderer::new(self.analysis).type_def_path(ty)?;
        match ty.id {
            TypeDefId::Struct(id) => {
                let Some(data) = target_ir.items().struct_data(id) else {
                    return Ok(None);
                };
                Ok(Some(DeclarationDetails {
                    kind: SymbolKind::Struct,
                    path,
                    signature: Some(renderer.struct_signature(data)),
                    docs: data.docs.as_ref().map(Documentation::text),
                }))
            }
            TypeDefId::Enum(id) => {
                let Some(data) = target_ir.items().enum_data(id) else {
                    return Ok(None);
                };
                Ok(Some(DeclarationDetails {
                    kind: SymbolKind::Enum,
                    path,
                    signature: Some(renderer.enum_signature(data)),
                    docs: data.docs.as_ref().map(Documentation::text),
                }))
            }
            TypeDefId::Union(id) => {
                let Some(data) = target_ir.items().union_data(id) else {
                    return Ok(None);
                };
                Ok(Some(DeclarationDetails {
                    kind: SymbolKind::Union,
                    path,
                    signature: Some(renderer.union_signature(data)),
                    docs: data.docs.as_ref().map(Documentation::text),
                }))
            }
        }
    }

    fn trait_details(&self, trait_ref: TraitRef) -> anyhow::Result<Option<DeclarationDetails>> {
        let Some(data) = self.analysis.semantic_ir.trait_data(trait_ref)? else {
            return Ok(None);
        };
        Ok(Some(DeclarationDetails {
            kind: SymbolKind::Trait,
            path: PathRenderer::new(self.analysis).trait_path(trait_ref)?,
            signature: Some(SignatureRenderer::new(self.analysis).trait_signature(data)),
            docs: data.docs.as_ref().map(Documentation::text),
        }))
    }

    fn function_details(
        &self,
        function: ResolvedFunctionRef,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        let members = MemberView::new(self.analysis);
        let Some(function) = members.function(function)? else {
            return Ok(None);
        };
        Ok(Some(DeclarationDetails {
            kind: function.symbol_kind(),
            path: function.display_path(&PathRenderer::new(self.analysis))?,
            signature: Some(
                SignatureRenderer::new(self.analysis).member_function_signature(&function),
            ),
            docs: function.docs_text(),
        }))
    }

    fn field_details(&self, field: ResolvedFieldRef) -> anyhow::Result<Option<DeclarationDetails>> {
        let members = MemberView::new(self.analysis);
        let Some(field) = members.field(field)? else {
            return Ok(None);
        };
        Ok(Some(DeclarationDetails {
            kind: SymbolKind::Field,
            path: field.display_path(&PathRenderer::new(self.analysis))?,
            signature: SignatureRenderer::new(self.analysis).member_field_signature(&field),
            docs: field.docs_text(),
        }))
    }

    fn enum_variant_details(
        &self,
        variant: ResolvedEnumVariantRef,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        match variant {
            ResolvedEnumVariantRef::Semantic(variant_ref) => {
                let Some(data) = self.analysis.semantic_ir.enum_variant_data(variant_ref)? else {
                    return Ok(None);
                };
                Ok(Some(DeclarationDetails {
                    kind: SymbolKind::EnumVariant,
                    path: PathRenderer::new(self.analysis).enum_variant_path(variant_ref)?,
                    signature: Some(
                        SignatureRenderer::new(self.analysis).enum_variant_signature(data),
                    ),
                    docs: data.variant.docs.as_ref().map(Documentation::text),
                }))
            }
            ResolvedEnumVariantRef::BodyLocal(variant_ref) => {
                let Some(data) = self.analysis.body_ir.local_enum_variant_data(variant_ref)? else {
                    return Ok(None);
                };
                Ok(Some(DeclarationDetails {
                    kind: SymbolKind::EnumVariant,
                    path: None,
                    signature: Some(
                        SignatureRenderer::new(self.analysis).local_enum_variant_signature(data),
                    ),
                    docs: data.variant.docs.as_ref().map(Documentation::text),
                }))
            }
        }
    }

    fn type_alias_details(
        &self,
        type_alias_ref: TypeAliasRef,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        let Some(data) = self.analysis.semantic_ir.type_alias_data(type_alias_ref)? else {
            return Ok(None);
        };
        Ok(Some(DeclarationDetails {
            kind: SymbolKind::TypeAlias,
            path: PathRenderer::new(self.analysis).type_alias_path(type_alias_ref)?,
            signature: Some(SignatureRenderer::new(self.analysis).type_alias_signature(data)),
            docs: data.docs.as_ref().map(Documentation::text),
        }))
    }

    fn const_details(&self, const_ref: ConstRef) -> anyhow::Result<Option<DeclarationDetails>> {
        let Some(data) = self.analysis.semantic_ir.const_data(const_ref)? else {
            return Ok(None);
        };
        Ok(Some(DeclarationDetails {
            kind: SymbolKind::Const,
            path: PathRenderer::new(self.analysis).const_path(const_ref)?,
            signature: Some(SignatureRenderer::new(self.analysis).const_signature(data)),
            docs: data.docs.as_ref().map(Documentation::text),
        }))
    }

    fn static_details(&self, static_ref: StaticRef) -> anyhow::Result<Option<DeclarationDetails>> {
        let Some(data) = self.analysis.semantic_ir.static_data(static_ref)? else {
            return Ok(None);
        };
        Ok(Some(DeclarationDetails {
            kind: SymbolKind::Static,
            path: PathRenderer::new(self.analysis).static_path(static_ref)?,
            signature: Some(SignatureRenderer::new(self.analysis).static_signature(data)),
            docs: data.docs.as_ref().map(Documentation::text),
        }))
    }

    fn module_details(
        &self,
        module_ref: ModuleRef,
        context: &DeclarationDetailsContext,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        let Some(module) = self.analysis.def_map.module(module_ref)? else {
            return Ok(None);
        };
        let name = context
            .module_display_name
            .as_deref()
            .or(module.name.as_deref())
            .unwrap_or("crate");
        Ok(Some(DeclarationDetails {
            kind: SymbolKind::Module,
            path: PathRenderer::new(self.analysis).module_path(module_ref)?,
            signature: Some(format!("mod {name}")),
            docs: module.docs.as_ref().map(Documentation::text),
        }))
    }

    fn local_def_details(
        &self,
        local_def: LocalDefRef,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        let Some(data) = self.analysis.def_map.local_def(local_def)? else {
            return Ok(None);
        };
        let path = PathRenderer::new(self.analysis)
            .module_path(ModuleRef {
                target: local_def.target,
                module: data.module,
            })?
            .map(|module| format!("{module}::{}", data.name));
        Ok(Some(DeclarationDetails {
            kind: SymbolKind::from_local_def_kind(data.kind),
            path,
            signature: Some(format!("{} {}", data.kind, data.name)),
            docs: None,
        }))
    }
}
