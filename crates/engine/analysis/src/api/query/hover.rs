//! Builds hover payloads from resolved analysis declarations.

use rg_body_ir::{
    BodyDeclarationRef, BodyTy, ResolvedEnumVariantRef, ResolvedFieldRef, ResolvedFunctionRef,
};
use rg_def_map::{LocalDefRef, ModuleRef, TargetRef};
use rg_parse::{FileId, Span};
use rg_semantic_ir::{
    ConstRef, Documentation, SemanticDeclarationRef, SemanticItemRef, StaticRef, TraitRef,
    TypeAliasRef, TypeDefId, TypeDefRef,
};

use crate::{
    api::{
        Analysis,
        query::type_at::TypeResolver,
        render::{path::PathRenderer, signature::SignatureRenderer},
        resolve::declaration::SymbolDeclarationResolver,
        view::{
            declaration::{Declaration, DeclarationRef, DeclarationView},
            member::MemberLookup,
        },
    },
    model::{HoverBlock, HoverInfo, SymbolAt, SymbolKind},
};

pub(crate) struct HoverResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> HoverResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn hover(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<HoverInfo>> {
        let Some(symbol) = self.0.symbol_at_for_query(target, file_id, offset)? else {
            return Ok(None);
        };
        let range = self.symbol_range(&symbol)?;
        let declarations =
            SymbolDeclarationResolver::new(self.0).declarations_for_symbol(symbol.clone())?;
        let module_display_name = Self::module_display_name_for_symbol(&symbol);
        let mut blocks = Vec::new();

        for declaration in declarations {
            let block = self.hover_for_declaration(declaration, module_display_name.clone())?;
            let Some(block) = block else {
                continue;
            };
            if !blocks.contains(&block) {
                blocks.push(block);
            }
        }

        if blocks.is_empty()
            && let Some(ty) = TypeResolver::new(self.0).type_at(target, file_id, offset)?
            && let Some(block) = self.hover_for_ty(&ty)?
        {
            blocks.push(block);
        }

        Ok((!blocks.is_empty()).then_some(HoverInfo { range, blocks }))
    }

    fn module_display_name_for_symbol(symbol: &SymbolAt) -> Option<String> {
        match symbol {
            SymbolAt::BodyPath { path, .. }
            | SymbolAt::BodyValuePath { path, .. }
            | SymbolAt::TypePath { path, .. }
            | SymbolAt::UsePath { path, .. } => path.last_segment_label(),
            SymbolAt::Body { .. }
            | SymbolAt::Binding { .. }
            | SymbolAt::Def { .. }
            | SymbolAt::Expr { .. }
            | SymbolAt::Field { .. }
            | SymbolAt::Function { .. }
            | SymbolAt::EnumVariant { .. }
            | SymbolAt::LocalEnumVariant { .. }
            | SymbolAt::LocalItem { .. }
            | SymbolAt::LocalValueItem { .. }
            | SymbolAt::LocalField { .. }
            | SymbolAt::LocalFunction { .. } => None,
        }
    }

    fn hover_for_declaration(
        &self,
        declaration: DeclarationRef,
        module_display_name: Option<String>,
    ) -> anyhow::Result<Option<HoverBlock>> {
        match declaration {
            DeclarationRef::Module(module) => self.hover_for_module(module, module_display_name),
            DeclarationRef::LocalDef(local_def) => self.hover_for_local_def(local_def),
            DeclarationRef::Semantic(declaration) => {
                self.hover_for_semantic_declaration(declaration)
            }
            DeclarationRef::Body(declaration) => self.hover_for_body_declaration(declaration),
        }
    }

    fn hover_for_body_declaration(
        &self,
        declaration: BodyDeclarationRef,
    ) -> anyhow::Result<Option<HoverBlock>> {
        match declaration {
            BodyDeclarationRef::Binding(_) => {
                let Some(view) = self.0.body_ir.body_declaration_view(declaration)? else {
                    return Ok(None);
                };
                let Some(binding_data) = view.binding_data() else {
                    return Ok(None);
                };
                Ok(Some(HoverBlock {
                    kind: SymbolKind::Variable,
                    path: None,
                    signature: Some(
                        SignatureRenderer::new(self.0).binding_signature(binding_data)?,
                    ),
                    ty: None,
                    docs: None,
                }))
            }
            BodyDeclarationRef::Item(_) => {
                let Some(view) = self.0.body_ir.body_declaration_view(declaration)? else {
                    return Ok(None);
                };
                let Some(item) = view.item_data() else {
                    return Ok(None);
                };
                let Some(declaration) = self.declaration(declaration)? else {
                    return Ok(None);
                };
                Ok(Some(HoverBlock {
                    kind: declaration.kind(),
                    path: None,
                    signature: Some(SignatureRenderer::new(self.0).local_item_signature(item)),
                    ty: None,
                    docs: item.docs.as_ref().map(Documentation::text),
                }))
            }
            BodyDeclarationRef::ValueItem(_) => {
                let Some(view) = self.0.body_ir.body_declaration_view(declaration)? else {
                    return Ok(None);
                };
                let Some(item) = view.value_item_data() else {
                    return Ok(None);
                };
                let Some(declaration) = self.declaration(declaration)? else {
                    return Ok(None);
                };
                Ok(Some(HoverBlock {
                    kind: declaration.kind(),
                    path: None,
                    signature: Some(
                        SignatureRenderer::new(self.0).local_value_item_signature(item),
                    ),
                    ty: None,
                    docs: item.docs.as_ref().map(Documentation::text),
                }))
            }
            BodyDeclarationRef::Function(function) => {
                self.hover_for_function(ResolvedFunctionRef::BodyLocal(function))
            }
            BodyDeclarationRef::Field(field) => {
                self.hover_for_field(ResolvedFieldRef::BodyLocal(field))
            }
            BodyDeclarationRef::EnumVariant(variant) => {
                self.hover_for_enum_variant(ResolvedEnumVariantRef::BodyLocal(variant))
            }
            BodyDeclarationRef::Impl(_) => Ok(None),
        }
    }

    fn hover_for_semantic_item(&self, item: SemanticItemRef) -> anyhow::Result<Option<HoverBlock>> {
        match item {
            SemanticItemRef::TypeDef(ty) => self.hover_for_type_def(ty),
            SemanticItemRef::Trait(trait_ref) => self.hover_for_trait(trait_ref),
            SemanticItemRef::Impl(_) => Ok(None),
            SemanticItemRef::Function(function) => {
                self.hover_for_function(ResolvedFunctionRef::Semantic(function))
            }
            SemanticItemRef::TypeAlias(type_alias_ref) => self.hover_for_type_alias(type_alias_ref),
            SemanticItemRef::Const(const_ref) => self.hover_for_const(const_ref),
            SemanticItemRef::Static(static_ref) => self.hover_for_static(static_ref),
        }
    }

    fn hover_for_semantic_declaration(
        &self,
        declaration: SemanticDeclarationRef,
    ) -> anyhow::Result<Option<HoverBlock>> {
        match declaration {
            SemanticDeclarationRef::Item(item) => self.hover_for_semantic_item(item),
            SemanticDeclarationRef::Field(field) => {
                self.hover_for_field(ResolvedFieldRef::Semantic(field))
            }
            SemanticDeclarationRef::EnumVariant(variant) => {
                self.hover_for_enum_variant(ResolvedEnumVariantRef::Semantic(variant))
            }
        }
    }

    fn hover_for_type_def(&self, ty: TypeDefRef) -> anyhow::Result<Option<HoverBlock>> {
        let Some(target_ir) = self.0.semantic_ir.target_ir(ty.target)? else {
            return Ok(None);
        };
        let renderer = SignatureRenderer::new(self.0);
        let path = PathRenderer::new(self.0).type_def_path(ty)?;
        match ty.id {
            TypeDefId::Struct(id) => {
                let Some(data) = target_ir.items().struct_data(id) else {
                    return Ok(None);
                };
                Ok(Some(HoverBlock {
                    kind: SymbolKind::Struct,
                    path,
                    signature: Some(renderer.struct_signature(data)),
                    ty: None,
                    docs: data.docs.as_ref().map(Documentation::text),
                }))
            }
            TypeDefId::Enum(id) => {
                let Some(data) = target_ir.items().enum_data(id) else {
                    return Ok(None);
                };
                Ok(Some(HoverBlock {
                    kind: SymbolKind::Enum,
                    path,
                    signature: Some(renderer.enum_signature(data)),
                    ty: None,
                    docs: data.docs.as_ref().map(Documentation::text),
                }))
            }
            TypeDefId::Union(id) => {
                let Some(data) = target_ir.items().union_data(id) else {
                    return Ok(None);
                };
                Ok(Some(HoverBlock {
                    kind: SymbolKind::Union,
                    path,
                    signature: Some(renderer.union_signature(data)),
                    ty: None,
                    docs: data.docs.as_ref().map(Documentation::text),
                }))
            }
        }
    }

    fn hover_for_trait(&self, trait_ref: TraitRef) -> anyhow::Result<Option<HoverBlock>> {
        let Some(data) = self.0.semantic_ir.trait_data(trait_ref)? else {
            return Ok(None);
        };
        Ok(Some(HoverBlock {
            kind: SymbolKind::Trait,
            path: PathRenderer::new(self.0).trait_path(trait_ref)?,
            signature: Some(SignatureRenderer::new(self.0).trait_signature(data)),
            ty: None,
            docs: data.docs.as_ref().map(Documentation::text),
        }))
    }

    fn hover_for_function(
        &self,
        function: ResolvedFunctionRef,
    ) -> anyhow::Result<Option<HoverBlock>> {
        let members = MemberLookup::new(self.0);
        let Some(function) = members.function_view(function)? else {
            return Ok(None);
        };
        let path = function.display_path(&PathRenderer::new(self.0))?;

        Ok(Some(HoverBlock {
            kind: function.symbol_kind(),
            path,
            signature: Some(SignatureRenderer::new(self.0).member_function_signature(&function)),
            ty: None,
            docs: function.docs_text(),
        }))
    }

    fn hover_for_field(&self, field: ResolvedFieldRef) -> anyhow::Result<Option<HoverBlock>> {
        let members = MemberLookup::new(self.0);
        let Some(field) = members.field_view(field)? else {
            return Ok(None);
        };
        let path = field.display_path(&PathRenderer::new(self.0))?;

        Ok(Some(HoverBlock {
            kind: SymbolKind::Field,
            path,
            signature: SignatureRenderer::new(self.0).member_field_signature(&field),
            ty: None,
            docs: field.docs_text(),
        }))
    }

    fn hover_for_enum_variant(
        &self,
        variant: ResolvedEnumVariantRef,
    ) -> anyhow::Result<Option<HoverBlock>> {
        match variant {
            ResolvedEnumVariantRef::Semantic(variant_ref) => {
                let Some(data) = self.0.semantic_ir.enum_variant_data(variant_ref)? else {
                    return Ok(None);
                };
                Ok(Some(HoverBlock {
                    kind: SymbolKind::EnumVariant,
                    path: PathRenderer::new(self.0).enum_variant_path(variant_ref)?,
                    signature: Some(SignatureRenderer::new(self.0).enum_variant_signature(data)),
                    ty: None,
                    docs: data.variant.docs.as_ref().map(Documentation::text),
                }))
            }
            ResolvedEnumVariantRef::BodyLocal(variant_ref) => {
                let Some(data) = self.0.body_ir.local_enum_variant_data(variant_ref)? else {
                    return Ok(None);
                };
                Ok(Some(HoverBlock {
                    kind: SymbolKind::EnumVariant,
                    path: None,
                    signature: Some(
                        SignatureRenderer::new(self.0).local_enum_variant_signature(data),
                    ),
                    ty: None,
                    docs: data.variant.docs.as_ref().map(Documentation::text),
                }))
            }
        }
    }

    fn hover_for_type_alias(
        &self,
        type_alias_ref: TypeAliasRef,
    ) -> anyhow::Result<Option<HoverBlock>> {
        let Some(data) = self.0.semantic_ir.type_alias_data(type_alias_ref)? else {
            return Ok(None);
        };
        Ok(Some(HoverBlock {
            kind: SymbolKind::TypeAlias,
            path: PathRenderer::new(self.0).type_alias_path(type_alias_ref)?,
            signature: Some(SignatureRenderer::new(self.0).type_alias_signature(data)),
            ty: None,
            docs: data.docs.as_ref().map(Documentation::text),
        }))
    }

    fn hover_for_const(&self, const_ref: ConstRef) -> anyhow::Result<Option<HoverBlock>> {
        let Some(data) = self.0.semantic_ir.const_data(const_ref)? else {
            return Ok(None);
        };
        Ok(Some(HoverBlock {
            kind: SymbolKind::Const,
            path: PathRenderer::new(self.0).const_path(const_ref)?,
            signature: Some(SignatureRenderer::new(self.0).const_signature(data)),
            ty: None,
            docs: data.docs.as_ref().map(Documentation::text),
        }))
    }

    fn hover_for_static(&self, static_ref: StaticRef) -> anyhow::Result<Option<HoverBlock>> {
        let Some(data) = self.0.semantic_ir.static_data(static_ref)? else {
            return Ok(None);
        };
        Ok(Some(HoverBlock {
            kind: SymbolKind::Static,
            path: PathRenderer::new(self.0).static_path(static_ref)?,
            signature: Some(SignatureRenderer::new(self.0).static_signature(data)),
            ty: None,
            docs: data.docs.as_ref().map(Documentation::text),
        }))
    }

    fn hover_for_module(
        &self,
        module_ref: ModuleRef,
        display_name: Option<String>,
    ) -> anyhow::Result<Option<HoverBlock>> {
        let Some(module) = self.0.def_map.module(module_ref)? else {
            return Ok(None);
        };
        let name = display_name
            .as_deref()
            .or(module.name.as_deref())
            .unwrap_or("crate");
        Ok(Some(HoverBlock {
            kind: SymbolKind::Module,
            path: PathRenderer::new(self.0).module_path(module_ref)?,
            signature: Some(format!("mod {name}")),
            ty: None,
            docs: module.docs.as_ref().map(Documentation::text),
        }))
    }

    fn hover_for_local_def(&self, local_def: LocalDefRef) -> anyhow::Result<Option<HoverBlock>> {
        let Some(data) = self.0.def_map.local_def(local_def)? else {
            return Ok(None);
        };
        let path = PathRenderer::new(self.0)
            .module_path(ModuleRef {
                target: local_def.target,
                module: data.module,
            })?
            .map(|module| format!("{module}::{}", data.name));
        Ok(Some(HoverBlock {
            kind: SymbolKind::from_local_def_kind(data.kind),
            path,
            signature: Some(format!("{} {}", data.kind, data.name)),
            ty: None,
            docs: None,
        }))
    }

    fn hover_for_ty(&self, ty: &BodyTy) -> anyhow::Result<Option<HoverBlock>> {
        let Some(signature) = SignatureRenderer::new(self.0).ty_signature(ty)? else {
            return Ok(None);
        };
        Ok(Some(HoverBlock {
            kind: SymbolKind::TypeAlias,
            path: None,
            signature: None,
            ty: Some(signature),
            docs: None,
        }))
    }

    fn declaration(
        &self,
        declaration: impl Into<DeclarationRef>,
    ) -> anyhow::Result<Option<Declaration>> {
        DeclarationView::new(self.0).declaration(declaration.into())
    }

    fn symbol_range(&self, symbol: &SymbolAt) -> anyhow::Result<Option<Span>> {
        match symbol {
            SymbolAt::Body { body } => Ok(self
                .0
                .body_ir
                .body_data(*body)?
                .map(|body_data| body_data.source().span)),
            SymbolAt::Binding { body, binding } => Ok(self
                .0
                .body_ir
                .body_data(*body)?
                .and_then(|body_data| body_data.binding(*binding))
                .map(|binding| binding.source.span)),
            SymbolAt::BodyPath { span, .. }
            | SymbolAt::BodyValuePath { span, .. }
            | SymbolAt::Def { span, .. }
            | SymbolAt::Field { span, .. }
            | SymbolAt::Function { span, .. }
            | SymbolAt::EnumVariant { span, .. }
            | SymbolAt::LocalItem { span, .. }
            | SymbolAt::LocalValueItem { span, .. }
            | SymbolAt::LocalField { span, .. }
            | SymbolAt::LocalEnumVariant { span, .. }
            | SymbolAt::LocalFunction { span, .. }
            | SymbolAt::TypePath { span, .. }
            | SymbolAt::UsePath { span, .. } => Ok(Some(*span)),
            SymbolAt::Expr { body, expr } => Ok(self
                .0
                .body_ir
                .body_data(*body)?
                .and_then(|body_data| body_data.expr(*expr))
                .map(|expr| expr.source.span)),
        }
    }
}
