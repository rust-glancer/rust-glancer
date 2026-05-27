//! Composite declaration details used by editor features.
//!
//! Declarations identify source facts, but UI features usually need the surrounding presentation
//! facts as well: docs, display path, symbol kind, and a compact signature. This view keeps that
//! storage-specific projection out of feature queries.

use rg_ir_model::{
    BodyDeclarationRef, ConstRef, LocalDefRef, ModuleRef, SemanticItemRef, StaticRef, TraitRef,
    TypeAliasRef, TypeDefId, TypeDefRef,
    identity::{
        DeclarationRef, DeclarationRefRepr, EnumVariantRef, EnumVariantRefRepr, FieldRef,
        FunctionRef, ItemRef, ItemRefRepr, NameDefRefRepr,
    },
};
use rg_semantic_ir::Documentation;

use crate::{IndexedSymbolKind, IndexedViewDb, path::PathView, signature::SignatureRenderer};

use super::{declaration::DeclarationView, member::MemberView};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeclarationDetailsContext {
    pub module_display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclarationDetails {
    pub kind: IndexedSymbolKind,
    pub path: Option<String>,
    pub signature: Option<String>,
    pub docs: Option<String>,
}

pub struct DeclarationDetailsView<'a, 'db> {
    analysis: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> DeclarationDetailsView<'a, 'db> {
    pub fn new(analysis: &'a IndexedViewDb<'db>) -> Self {
        Self { analysis }
    }

    pub fn details_for_declaration(
        &self,
        declaration: DeclarationRef,
        context: &DeclarationDetailsContext,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        match declaration.repr() {
            DeclarationRefRepr::Module(module) => self.module_details(module, context),
            DeclarationRefRepr::NameDef(name_def) => match name_def.repr() {
                NameDefRefRepr::DefMapLocal(local_def) => self.local_def_details(local_def),
            },
            DeclarationRefRepr::Item(item) => self.item_details(item),
            DeclarationRefRepr::Function(function) => self.function_details(function),
            DeclarationRefRepr::Field(field) => self.field_details(field),
            DeclarationRefRepr::EnumVariant(variant) => self.enum_variant_details(variant),
            DeclarationRefRepr::Binding(binding) => {
                self.body_declaration_details(BodyDeclarationRef::Binding(binding.body_ir()))
            }
            DeclarationRefRepr::Impl(_) => Ok(None),
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
                    kind: IndexedSymbolKind::Variable,
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
                let Some(declaration_view) = DeclarationView::new(self.analysis)
                    .declaration(DeclarationRef::body(declaration))?
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
                let Some(declaration_view) = DeclarationView::new(self.analysis)
                    .declaration(DeclarationRef::body(declaration))?
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
                self.function_details(FunctionRef::body_local(function))
            }
            BodyDeclarationRef::Field(field) => self.field_details(FieldRef::body_local(field)),
            BodyDeclarationRef::EnumVariant(variant) => {
                self.enum_variant_details(EnumVariantRef::body_local(variant))
            }
            BodyDeclarationRef::Impl(_) => Ok(None),
        }
    }

    fn item_details(&self, item: ItemRef) -> anyhow::Result<Option<DeclarationDetails>> {
        match item.repr() {
            ItemRefRepr::Semantic(item) => self.semantic_item_details(item),
            ItemRefRepr::BodyLocal(item) => {
                self.body_declaration_details(BodyDeclarationRef::Item(item))
            }
            ItemRefRepr::BodyLocalValue(item) => {
                self.body_declaration_details(BodyDeclarationRef::ValueItem(item))
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
                self.function_details(FunctionRef::semantic(function))
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
        let path = PathView::new(self.analysis).type_def_path(ty)?;
        match ty.id {
            TypeDefId::Struct(id) => {
                let Some(data) = target_ir.items().struct_data(id) else {
                    return Ok(None);
                };
                Ok(Some(DeclarationDetails {
                    kind: IndexedSymbolKind::Struct,
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
                    kind: IndexedSymbolKind::Enum,
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
                    kind: IndexedSymbolKind::Union,
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
            kind: IndexedSymbolKind::Trait,
            path: PathView::new(self.analysis).trait_path(trait_ref)?,
            signature: Some(SignatureRenderer::new(self.analysis).trait_signature(data)),
            docs: data.docs.as_ref().map(Documentation::text),
        }))
    }

    fn function_details(
        &self,
        function: FunctionRef,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        let members = MemberView::new(self.analysis);
        let Some(function) = members.function(function)? else {
            return Ok(None);
        };
        Ok(Some(DeclarationDetails {
            kind: function.symbol_kind(),
            path: function.display_path(&PathView::new(self.analysis))?,
            signature: Some(
                SignatureRenderer::new(self.analysis).member_function_signature(&function),
            ),
            docs: function.docs_text(),
        }))
    }

    fn field_details(&self, field: FieldRef) -> anyhow::Result<Option<DeclarationDetails>> {
        let members = MemberView::new(self.analysis);
        let Some(field) = members.field(field)? else {
            return Ok(None);
        };
        Ok(Some(DeclarationDetails {
            kind: IndexedSymbolKind::Field,
            path: field.display_path(&PathView::new(self.analysis))?,
            signature: SignatureRenderer::new(self.analysis).member_field_signature(&field),
            docs: field.docs_text(),
        }))
    }

    fn enum_variant_details(
        &self,
        variant: EnumVariantRef,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        match variant.repr() {
            EnumVariantRefRepr::Semantic(variant_ref) => {
                let Some(data) = self.analysis.semantic_ir.enum_variant_data(variant_ref)? else {
                    return Ok(None);
                };
                Ok(Some(DeclarationDetails {
                    kind: IndexedSymbolKind::EnumVariant,
                    path: PathView::new(self.analysis).enum_variant_path(variant_ref)?,
                    signature: Some(
                        SignatureRenderer::new(self.analysis).enum_variant_signature(data),
                    ),
                    docs: data.variant.docs.as_ref().map(Documentation::text),
                }))
            }
            EnumVariantRefRepr::BodyLocal(variant_ref) => {
                let Some(data) = self.analysis.body_ir.local_enum_variant_data(variant_ref)? else {
                    return Ok(None);
                };
                Ok(Some(DeclarationDetails {
                    kind: IndexedSymbolKind::EnumVariant,
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
            kind: IndexedSymbolKind::TypeAlias,
            path: PathView::new(self.analysis).type_alias_path(type_alias_ref)?,
            signature: Some(SignatureRenderer::new(self.analysis).type_alias_signature(data)),
            docs: data.docs.as_ref().map(Documentation::text),
        }))
    }

    fn const_details(&self, const_ref: ConstRef) -> anyhow::Result<Option<DeclarationDetails>> {
        let Some(data) = self.analysis.semantic_ir.const_data(const_ref)? else {
            return Ok(None);
        };
        Ok(Some(DeclarationDetails {
            kind: IndexedSymbolKind::Const,
            path: PathView::new(self.analysis).const_path(const_ref)?,
            signature: Some(SignatureRenderer::new(self.analysis).const_signature(data)),
            docs: data.docs.as_ref().map(Documentation::text),
        }))
    }

    fn static_details(&self, static_ref: StaticRef) -> anyhow::Result<Option<DeclarationDetails>> {
        let Some(data) = self.analysis.semantic_ir.static_data(static_ref)? else {
            return Ok(None);
        };
        Ok(Some(DeclarationDetails {
            kind: IndexedSymbolKind::Static,
            path: PathView::new(self.analysis).static_path(static_ref)?,
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
            kind: IndexedSymbolKind::Module,
            path: PathView::new(self.analysis).module_path(module_ref)?,
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
        let path = PathView::new(self.analysis)
            .module_path(ModuleRef {
                target: local_def.target,
                module: data.module,
            })?
            .map(|module| format!("{module}::{}", data.name));
        Ok(Some(DeclarationDetails {
            kind: IndexedSymbolKind::from_local_def_kind(data.kind),
            path,
            signature: Some(format!("{} {}", data.kind, data.name)),
            docs: None,
        }))
    }
}
