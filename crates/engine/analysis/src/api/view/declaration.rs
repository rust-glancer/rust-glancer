//! Source-level declaration lookup shared by editor queries.

use rg_body_ir::{
    BodyBindingRef, BodyDeclarationRef, BodyEnumVariantRef, BodyFieldRef, BodyFunctionRef,
    BodyImplRef, BodyItemRef, BodyValueItemRef, ResolvedDeclarationRef,
};
use rg_def_map::{DefId, LocalDefRef, ModuleOrigin, ModuleRef, TargetRef};
use rg_parse::{FileId, Span};
use rg_semantic_ir::{
    ConstRef, EnumVariantRef, FieldRef, FunctionRef, ImplRef, SemanticDeclarationRef,
    SemanticItemKind, SemanticItemRef, StaticRef, TraitRef, TypeAliasRef, TypeDefRef, TypeRef,
};

use crate::{
    api::{Analysis, view::member::MemberView},
    model::{
        DocumentSymbol, MemberFieldRef, MemberFunctionRef, NavigationTarget, NavigationTargetKind,
        SymbolKind,
    },
};

/// Storage-independent identity for declarations that editor features can project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::From)]
pub(crate) enum DeclarationRef {
    #[from]
    Module(ModuleRef),
    #[from]
    LocalDef(LocalDefRef),
    #[from(
        SemanticDeclarationRef,
        SemanticItemRef,
        TypeDefRef,
        TraitRef,
        ImplRef,
        FunctionRef,
        TypeAliasRef,
        ConstRef,
        StaticRef,
        FieldRef,
        EnumVariantRef
    )]
    Semantic(SemanticDeclarationRef),
    #[from(
        BodyDeclarationRef,
        BodyBindingRef,
        BodyItemRef,
        BodyValueItemRef,
        BodyImplRef,
        BodyFieldRef,
        BodyEnumVariantRef,
        BodyFunctionRef
    )]
    Body(BodyDeclarationRef),
}

impl From<ResolvedDeclarationRef> for DeclarationRef {
    fn from(declaration: ResolvedDeclarationRef) -> Self {
        match declaration {
            ResolvedDeclarationRef::Def(DefId::Module(module)) => module.into(),
            ResolvedDeclarationRef::Def(DefId::Local(local_def)) => local_def.into(),
            ResolvedDeclarationRef::Semantic(declaration) => declaration.into(),
            ResolvedDeclarationRef::Body(declaration) => declaration.into(),
        }
    }
}

/// Composite declaration facts shared by editor queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Declaration {
    target: TargetRef,
    kind: SymbolKind,
    name: String,
    file_id: FileId,
    span: Span,
    selection_span: Span,
}

impl Declaration {
    pub(crate) fn new(
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

    pub(crate) fn target(&self) -> TargetRef {
        self.target
    }

    pub(crate) fn kind(&self) -> SymbolKind {
        self.kind
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn file_id(&self) -> FileId {
        self.file_id
    }

    pub(crate) fn span(&self) -> Span {
        self.span
    }

    pub(crate) fn selection_span(&self) -> Span {
        self.selection_span
    }
}

/// Reads declaration facts for IDs that already identify one source declaration.
pub(crate) struct DeclarationView<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> DeclarationView<'a, 'db> {
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
            DeclarationRef::Semantic(declaration) => self.semantic_declaration(declaration),
            DeclarationRef::Body(declaration) => self.body_declaration(declaration),
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

    fn semantic_declaration(
        &self,
        declaration: SemanticDeclarationRef,
    ) -> anyhow::Result<Option<Declaration>> {
        match declaration {
            SemanticDeclarationRef::Item(item) => self.semantic_item(item),
            SemanticDeclarationRef::Field(field) => self.semantic_field(field),
            SemanticDeclarationRef::EnumVariant(variant) => self.semantic_enum_variant(variant),
        }
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

    fn body_declaration(
        &self,
        declaration: BodyDeclarationRef,
    ) -> anyhow::Result<Option<Declaration>> {
        let Some(view) = self.analysis.body_ir.body_declaration_view(declaration)? else {
            return Ok(None);
        };

        let target = declaration.body().target;
        let source = view.source();
        let selection_span = view.name_source().unwrap_or(source).span;

        let declaration = match declaration {
            BodyDeclarationRef::Binding(_) => Declaration {
                target,
                kind: SymbolKind::Variable,
                name: view
                    .name()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "<unsupported>".to_string()),
                file_id: source.file_id,
                span: source.span,
                selection_span,
            },
            BodyDeclarationRef::Item(_) => {
                let Some(kind) = view.item_kind() else {
                    return Ok(None);
                };
                let Some(name) = view.name() else {
                    return Ok(None);
                };
                Declaration {
                    target,
                    kind: SymbolKind::from_body_item_kind(kind),
                    name: name.to_string(),
                    file_id: source.file_id,
                    span: source.span,
                    selection_span,
                }
            }
            BodyDeclarationRef::ValueItem(_) => {
                let Some(kind) = view.value_item_kind() else {
                    return Ok(None);
                };
                let Some(name) = view.name() else {
                    return Ok(None);
                };
                Declaration {
                    target,
                    kind: SymbolKind::from_body_value_item_kind(kind),
                    name: name.to_string(),
                    file_id: source.file_id,
                    span: source.span,
                    selection_span,
                }
            }
            BodyDeclarationRef::Impl(_) => {
                let Some((self_ty, trait_ref)) = view.impl_header() else {
                    return Ok(None);
                };
                Declaration {
                    target,
                    kind: SymbolKind::Impl,
                    name: Self::impl_label(self_ty, trait_ref),
                    file_id: source.file_id,
                    span: source.span,
                    selection_span,
                }
            }
            BodyDeclarationRef::Field(_) => {
                let Some(data) = view.field_data() else {
                    return Ok(None);
                };
                Declaration {
                    target,
                    kind: SymbolKind::Field,
                    name: Self::field_label(data.field.key_declaration_label()),
                    file_id: source.file_id,
                    span: source.span,
                    selection_span,
                }
            }
            BodyDeclarationRef::EnumVariant(_) => {
                let Some(name) = view.name() else {
                    return Ok(None);
                };
                Declaration {
                    target,
                    kind: SymbolKind::EnumVariant,
                    name: name.to_string(),
                    file_id: source.file_id,
                    span: source.span,
                    selection_span,
                }
            }
            BodyDeclarationRef::Function(_) => {
                let Some(owner) = view.function_owner() else {
                    return Ok(None);
                };
                let Some(name) = view.name() else {
                    return Ok(None);
                };
                Declaration {
                    target,
                    kind: SymbolKind::from_body_function_owner(owner),
                    name: name.to_string(),
                    file_id: source.file_id,
                    span: source.span,
                    selection_span,
                }
            }
        };

        Ok(Some(declaration))
    }

    fn semantic_enum_variant(
        &self,
        variant_ref: EnumVariantRef,
    ) -> anyhow::Result<Option<Declaration>> {
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

    fn semantic_field(&self, field: FieldRef) -> anyhow::Result<Option<Declaration>> {
        Ok(MemberView::new(self.analysis)
            .field(MemberFieldRef::Semantic(field))?
            .and_then(|field| field.declaration()))
    }

    fn semantic_function(&self, function: FunctionRef) -> anyhow::Result<Option<Declaration>> {
        Ok(MemberView::new(self.analysis)
            .function(MemberFunctionRef::Semantic(function))?
            .map(|function| function.declaration()))
    }

    fn impl_label(self_ty: &TypeRef, trait_ref: Option<&TypeRef>) -> String {
        match trait_ref {
            Some(trait_ref) => format!("impl {trait_ref} for {self_ty}"),
            None => format!("impl {self_ty}"),
        }
    }

    fn field_label(name: Option<String>) -> String {
        name.unwrap_or_else(|| "<unsupported>".to_string())
    }
}

impl From<Declaration> for NavigationTarget {
    fn from(declaration: Declaration) -> Self {
        Self {
            target: declaration.target,
            kind: NavigationTargetKind::from(declaration.kind),
            name: declaration.name,
            file_id: declaration.file_id,
            span: Some(declaration.selection_span),
        }
    }
}

impl From<Declaration> for DocumentSymbol {
    fn from(declaration: Declaration) -> Self {
        Self {
            name: declaration.name,
            kind: declaration.kind,
            file_id: declaration.file_id,
            span: declaration.span,
            selection_span: declaration.selection_span,
            children: Vec::new(),
        }
    }
}
