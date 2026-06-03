//! Composite declaration details used by editor features.
//!
//! Declarations identify source facts, but UI features usually need surrounding presentation
//! facts as well: docs, display path, symbol kind, and a compact signature.

use rg_ir_model::{
    BodyBindingRef, ConstRef, EnumVariantRef, FieldRef, FunctionRef, LocalDefRef, ModuleRef,
    SemanticItemRef, StaticRef, TraitRef, TypeAliasRef, TypeDefId, TypeDefRef,
    identity::DeclarationRef,
};
use rg_ir_storage::{DefMapQuery, ItemStoreQuery};
use rg_ir_view::{IndexedViewDb, display::signature::SignatureRenderer, item::path::PathView};
use rg_item_tree::Documentation;

use crate::{SymbolKind, query::member::MemberView};

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

pub(crate) struct DeclarationDetailsResolver<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> DeclarationDetailsResolver<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub(crate) fn details_for_declaration(
        &self,
        declaration: DeclarationRef,
        context: &DeclarationDetailsContext,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        match declaration {
            DeclarationRef::Module(module) => self.module_details(module, context),
            DeclarationRef::LocalDef(local_def) => self.local_def_details(local_def),
            DeclarationRef::Item(item) => self.semantic_item_details(item),
            DeclarationRef::Field(field) => self.field_details(field),
            DeclarationRef::EnumVariant(variant) => self.enum_variant_details(variant),
            DeclarationRef::BodyBinding(binding) => self.body_binding_details(binding),
        }
    }

    fn body_binding_details(
        &self,
        binding_ref: BodyBindingRef,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        let Some(body) = self.db.body_data(binding_ref.body)? else {
            return Ok(None);
        };
        let Some(binding_data) = body.binding(binding_ref.binding) else {
            return Ok(None);
        };

        Ok(Some(DeclarationDetails {
            kind: SymbolKind::Variable,
            path: None,
            signature: Some(SignatureRenderer::binding_signature(self.db, binding_data)?),
            docs: None,
        }))
    }

    fn semantic_item_details(
        &self,
        item: SemanticItemRef,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        match item {
            SemanticItemRef::TypeDef(ty) => self.type_def_details(ty),
            SemanticItemRef::Trait(trait_ref) => self.trait_details(trait_ref),
            SemanticItemRef::Impl(_) => Ok(None),
            SemanticItemRef::Function(function) => self.function_details(function),
            SemanticItemRef::TypeAlias(type_alias_ref) => self.type_alias_details(type_alias_ref),
            SemanticItemRef::Const(const_ref) => self.const_details(const_ref),
            SemanticItemRef::Static(static_ref) => self.static_details(static_ref),
        }
    }

    fn type_def_details(&self, ty: TypeDefRef) -> anyhow::Result<Option<DeclarationDetails>> {
        let item_query = ItemStoreQuery::new(self.db);
        let Some(items) = item_query.item_store_for_origin(ty.origin)? else {
            return Ok(None);
        };
        let path = PathView::new(self.db).type_def_path(ty)?;
        match ty.id {
            TypeDefId::Struct(id) => {
                let Some(data) = items.struct_data(id) else {
                    return Ok(None);
                };
                Ok(Some(DeclarationDetails {
                    kind: SymbolKind::Struct,
                    path,
                    signature: Some(SignatureRenderer::struct_signature(data)),
                    docs: data.docs.as_ref().map(Documentation::text),
                }))
            }
            TypeDefId::Enum(id) => {
                let Some(data) = items.enum_data(id) else {
                    return Ok(None);
                };
                Ok(Some(DeclarationDetails {
                    kind: SymbolKind::Enum,
                    path,
                    signature: Some(SignatureRenderer::enum_signature(data)),
                    docs: data.docs.as_ref().map(Documentation::text),
                }))
            }
            TypeDefId::Union(id) => {
                let Some(data) = items.union_data(id) else {
                    return Ok(None);
                };
                Ok(Some(DeclarationDetails {
                    kind: SymbolKind::Union,
                    path,
                    signature: Some(SignatureRenderer::union_signature(data)),
                    docs: data.docs.as_ref().map(Documentation::text),
                }))
            }
        }
    }

    fn trait_details(&self, trait_ref: TraitRef) -> anyhow::Result<Option<DeclarationDetails>> {
        let Some(data) = ItemStoreQuery::new(self.db).trait_data(trait_ref)? else {
            return Ok(None);
        };
        Ok(Some(DeclarationDetails {
            kind: SymbolKind::Trait,
            path: PathView::new(self.db).trait_path(trait_ref)?,
            signature: Some(SignatureRenderer::trait_signature(data)),
            docs: data.docs.as_ref().map(Documentation::text),
        }))
    }

    fn function_details(
        &self,
        function: FunctionRef,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        let members = MemberView::new(self.db);
        let Some(function) = members.function(function)? else {
            return Ok(None);
        };
        Ok(Some(DeclarationDetails {
            kind: function.symbol_kind(),
            path: function.display_path(&PathView::new(self.db))?,
            signature: Some(SignatureRenderer::function_signature(function.data())),
            docs: function.docs_text(),
        }))
    }

    fn field_details(&self, field: FieldRef) -> anyhow::Result<Option<DeclarationDetails>> {
        let members = MemberView::new(self.db);
        let Some(field) = members.field(field)? else {
            return Ok(None);
        };
        Ok(Some(DeclarationDetails {
            kind: SymbolKind::Field,
            path: field.display_path(&PathView::new(self.db))?,
            signature: SignatureRenderer::field_signature(field.data()),
            docs: field.docs_text(),
        }))
    }

    fn enum_variant_details(
        &self,
        variant: EnumVariantRef,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        let Some(data) = ItemStoreQuery::new(self.db).enum_variant_data(variant)? else {
            return Ok(None);
        };
        Ok(Some(DeclarationDetails {
            kind: SymbolKind::EnumVariant,
            path: PathView::new(self.db).enum_variant_path(data)?,
            signature: Some(SignatureRenderer::enum_variant_signature(data.variant)),
            docs: data.variant.docs.as_ref().map(Documentation::text),
        }))
    }

    fn type_alias_details(
        &self,
        type_alias_ref: TypeAliasRef,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        let Some(data) = ItemStoreQuery::new(self.db).type_alias_data(type_alias_ref)? else {
            return Ok(None);
        };
        Ok(Some(DeclarationDetails {
            kind: SymbolKind::TypeAlias,
            path: PathView::new(self.db).type_alias_path(type_alias_ref)?,
            signature: Some(SignatureRenderer::type_alias_signature(data)),
            docs: data.docs.as_ref().map(Documentation::text),
        }))
    }

    fn const_details(&self, const_ref: ConstRef) -> anyhow::Result<Option<DeclarationDetails>> {
        let Some(data) = ItemStoreQuery::new(self.db).const_data(const_ref)? else {
            return Ok(None);
        };
        Ok(Some(DeclarationDetails {
            kind: SymbolKind::Const,
            path: PathView::new(self.db).const_path(const_ref)?,
            signature: Some(SignatureRenderer::const_signature(data)),
            docs: data.docs.as_ref().map(Documentation::text),
        }))
    }

    fn static_details(&self, static_ref: StaticRef) -> anyhow::Result<Option<DeclarationDetails>> {
        let Some(data) = ItemStoreQuery::new(self.db).static_data(static_ref)? else {
            return Ok(None);
        };
        Ok(Some(DeclarationDetails {
            kind: SymbolKind::Static,
            path: PathView::new(self.db).static_path(static_ref)?,
            signature: Some(SignatureRenderer::static_signature(data)),
            docs: data.docs.as_ref().map(Documentation::text),
        }))
    }

    fn module_details(
        &self,
        module_ref: ModuleRef,
        context: &DeclarationDetailsContext,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        let def_maps = DefMapQuery::new(self.db);
        let Some(module) = def_maps.module_data(module_ref)? else {
            return Ok(None);
        };
        let name = context
            .module_display_name
            .as_deref()
            .or(module.name.as_deref())
            .unwrap_or("crate");
        Ok(Some(DeclarationDetails {
            kind: SymbolKind::Module,
            path: PathView::new(self.db).module_path(module_ref)?,
            signature: Some(format!("mod {name}")),
            docs: module.docs.as_ref().map(Documentation::text),
        }))
    }

    fn local_def_details(
        &self,
        local_def_ref: LocalDefRef,
    ) -> anyhow::Result<Option<DeclarationDetails>> {
        let def_maps = DefMapQuery::new(self.db);
        let Some(data) = def_maps.local_def_data(local_def_ref)? else {
            return Ok(None);
        };
        let path = PathView::new(self.db)
            .module_path(ModuleRef {
                origin: local_def_ref.origin,
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
