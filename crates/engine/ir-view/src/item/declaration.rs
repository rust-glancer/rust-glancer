//! Source-level declaration lookup shared by editor queries.

use rg_def_map::{DefMapQuery, ModuleOrigin};
use rg_ir_model::{
    BodyBindingRef, EnumVariantRef, FieldRef, FunctionRef, LocalDefRef, ModuleRef,
    SemanticItemKind, SemanticItemRef, TargetRef, identity::DeclarationRef,
};
use rg_parse::{FileId, Span};
use rg_semantic_ir::{ItemStoreQuery, TypeRef};

use crate::{IndexedViewDb, SymbolKind, ty::member::MemberView};

/// Composite declaration facts shared by editor queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    target: TargetRef,
    kind: SymbolKind,
    name: String,
    file_id: FileId,
    span: Span,
    selection_span: Span,
}

impl Declaration {
    pub fn new(
        target: TargetRef,
        kind: SymbolKind,
        name: String,
        file_id: FileId,
        span: Span,
        selection_span: Span,
    ) -> Self {
        Self {
            target,
            kind,
            name,
            file_id,
            span,
            selection_span,
        }
    }

    pub fn target(&self) -> TargetRef {
        self.target
    }

    pub fn kind(&self) -> SymbolKind {
        self.kind
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn selection_span(&self) -> Span {
        self.selection_span
    }
}

/// Reads declaration facts for IDs that already identify one source declaration.
pub struct DeclarationView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> DeclarationView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub fn declaration(&self, declaration: DeclarationRef) -> anyhow::Result<Option<Declaration>> {
        match declaration {
            DeclarationRef::Module(module_ref) => self.module(module_ref),
            DeclarationRef::LocalDef(local_def) => self.local_def(local_def),
            DeclarationRef::Item(item) => self.semantic_item(item),
            DeclarationRef::Field(field) => self.semantic_field(field),
            DeclarationRef::EnumVariant(variant) => self.semantic_enum_variant(variant),
            DeclarationRef::BodyBinding(binding) => self.body_binding(binding),
        }
    }

    fn module(&self, module_ref: ModuleRef) -> anyhow::Result<Option<Declaration>> {
        let def_maps = DefMapQuery::new(self.db);
        let Some(module) = def_maps.module_data(module_ref)? else {
            return Ok(None);
        };
        let Some(name) = module.name.as_ref().map(ToString::to_string) else {
            return Ok(None);
        };
        let (file_id, span) = match module.origin {
            ModuleOrigin::Root { .. } | ModuleOrigin::Synthetic { .. } => return Ok(None),
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
            target: module_ref.origin.origin_target(),
            kind: SymbolKind::Module,
            name,
            file_id,
            span,
            selection_span: module.name_span.unwrap_or(span),
        }))
    }

    fn local_def(&self, local_def: LocalDefRef) -> anyhow::Result<Option<Declaration>> {
        let def_maps = DefMapQuery::new(self.db);
        let Some(data) = def_maps.local_def_data(local_def)? else {
            return Ok(None);
        };

        Ok(Some(Declaration {
            target: local_def.origin.origin_target(),
            kind: SymbolKind::from_local_def_kind(data.kind),
            name: data.name.to_string(),
            file_id: data.file_id,
            span: data.span,
            selection_span: data.name_span.unwrap_or(data.span),
        }))
    }

    fn semantic_item(&self, item: SemanticItemRef) -> anyhow::Result<Option<Declaration>> {
        let Some(view) = ItemStoreQuery::new(self.db).semantic_item_view(item)? else {
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
                let def_maps = DefMapQuery::new(self.db);
                let Some(local_impl) = def_maps.local_impl_data(local_impl_ref)? else {
                    return Ok(None);
                };
                let Some((self_ty, trait_ref)) = view.impl_header() else {
                    return Ok(None);
                };

                Ok(Some(Declaration {
                    target: item.origin().origin_target(),
                    kind: SymbolKind::Impl,
                    name: Self::impl_label(self_ty, trait_ref),
                    file_id: local_impl.file_id,
                    span: local_impl.span,
                    selection_span: local_impl.span,
                }))
            }
            SemanticItemKind::Function => match item {
                SemanticItemRef::Function(function) => self.semantic_function(function),
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
                    target: item.origin().origin_target(),
                    kind: SymbolKind::from_semantic_item_kind(view.kind()),
                    name: name.to_string(),
                    file_id: view.source().file_id,
                    span,
                    selection_span: view.name_span().unwrap_or(span),
                }))
            }
        }
    }

    fn body_binding(&self, binding_ref: BodyBindingRef) -> anyhow::Result<Option<Declaration>> {
        let Some(body) = self.db.body_ir.body_data(binding_ref.body)? else {
            return Ok(None);
        };
        let Some(binding) = body.binding(binding_ref.binding) else {
            return Ok(None);
        };

        Ok(Some(Declaration {
            target: binding_ref.body.target,
            kind: SymbolKind::Variable,
            name: binding
                .name
                .as_deref()
                .unwrap_or("<unsupported>")
                .to_string(),
            file_id: binding.source.file_id,
            span: binding.source.span,
            selection_span: binding.source.span,
        }))
    }

    fn semantic_enum_variant(
        &self,
        variant_ref: EnumVariantRef,
    ) -> anyhow::Result<Option<Declaration>> {
        let Some(data) = ItemStoreQuery::new(self.db).enum_variant_data(variant_ref)? else {
            return Ok(None);
        };

        Ok(Some(Declaration {
            target: variant_ref.origin.origin_target(),
            kind: SymbolKind::EnumVariant,
            name: data.variant.name.to_string(),
            file_id: data.file_id,
            span: data.variant.span,
            selection_span: data.variant.name_span,
        }))
    }

    fn semantic_field(&self, field: FieldRef) -> anyhow::Result<Option<Declaration>> {
        Ok(MemberView::new(self.db)
            .field(field)?
            .and_then(|field| field.declaration()))
    }

    fn semantic_function(&self, function: FunctionRef) -> anyhow::Result<Option<Declaration>> {
        Ok(MemberView::new(self.db)
            .function(function)?
            .map(|function| function.declaration()))
    }

    fn impl_label(self_ty: &TypeRef, trait_ref: Option<&TypeRef>) -> String {
        match trait_ref {
            Some(trait_ref) => format!("impl {trait_ref} for {self_ty}"),
            None => format!("impl {self_ty}"),
        }
    }
}
