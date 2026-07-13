//! Source-level declaration lookup shared by editor queries.

use std::{borrow::Cow, fmt};

use anyhow::Context as _;
use rg_def_map::{DefMapSource, ModuleOrigin};
use rg_ir_model::items::{FieldKey, TypeRef};
use rg_ir_model::{
    BodyBindingRef, CrateRef, EnumVariantRef, FieldRef, FunctionRef, ItemOwner, LocalDefRef,
    ModuleRef, SemanticItemKind, SemanticItemRef, identity::DeclarationRef,
};
use rg_parse::{FileId, Span};
use rg_semantic_ir::ItemStoreQuery;
use rg_text::Name;

use crate::{
    IndexedViewDb, SymbolKind,
    display::syntax::{NameDisplay, SyntaxRenderer},
};

/// The semantic or structural label carried by one declaration projection.
///
/// Most declarations keep their interned semantic name. Tuple fields retain their index, while
/// impls retain the stable item identity used to borrow their header only during presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DeclarationLabel {
    Name(Name),
    TupleField(usize),
    Unsupported,
    Impl(SemanticItemRef),
}

/// Composite declaration facts shared by editor queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    crate_ref: CrateRef,
    kind: SymbolKind,
    label: DeclarationLabel,
    file_id: FileId,
    span: Span,
    selection_span: Span,
}

impl Declaration {
    fn named(
        crate_ref: CrateRef,
        kind: SymbolKind,
        name: Name,
        file_id: FileId,
        span: Span,
        selection_span: Span,
    ) -> Self {
        Self {
            crate_ref,
            kind,
            label: DeclarationLabel::Name(name),
            file_id,
            span,
            selection_span,
        }
    }

    fn tuple_field(
        crate_ref: CrateRef,
        kind: SymbolKind,
        index: usize,
        file_id: FileId,
        span: Span,
        selection_span: Span,
    ) -> Self {
        Self {
            crate_ref,
            kind,
            label: DeclarationLabel::TupleField(index),
            file_id,
            span,
            selection_span,
        }
    }

    pub fn crate_ref(&self) -> CrateRef {
        self.crate_ref
    }

    pub fn kind(&self) -> SymbolKind {
        self.kind
    }

    /// Returns the canonical identifier identity when this declaration actually has one.
    pub fn semantic_name(&self) -> Option<&Name> {
        match &self.label {
            DeclarationLabel::Name(name) => Some(name),
            DeclarationLabel::TupleField(_)
            | DeclarationLabel::Unsupported
            | DeclarationLabel::Impl(_) => None,
        }
    }

    /// Returns the canonical text used for name-based search.
    ///
    /// Anonymous impl containers deliberately have no searchable name.
    pub fn search_name(&self) -> Option<Cow<'_, str>> {
        match &self.label {
            DeclarationLabel::Name(name) => Some(Cow::Borrowed(name)),
            DeclarationLabel::TupleField(index) => Some(Cow::Owned(format!("#{index}"))),
            DeclarationLabel::Unsupported => Some(Cow::Borrowed("<unsupported>")),
            DeclarationLabel::Impl(_) => None,
        }
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

/// Borrowed edition-aware presentation of one declaration label.
pub enum DeclarationDisplayName<'a> {
    Name(NameDisplay<'a>),
    TupleField(usize),
    Unsupported,
    Impl {
        syntax: SyntaxRenderer,
        self_ty: &'a TypeRef,
        trait_ref: Option<&'a TypeRef>,
    },
}

impl fmt::Display for DeclarationDisplayName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(name) => name.fmt(f),
            Self::TupleField(index) => write!(f, "#{index}"),
            Self::Unsupported => f.write_str("<unsupported>"),
            Self::Impl {
                syntax,
                self_ty,
                trait_ref,
            } => match trait_ref {
                Some(trait_ref) => write!(
                    f,
                    "impl {} for {}",
                    syntax.type_ref(trait_ref),
                    syntax.type_ref(self_ty)
                ),
                None => write!(f, "impl {}", syntax.type_ref(self_ty)),
            },
        }
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

    /// Return source facts for one declaration ref.
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

    /// Creates the declaration-site Rust-syntax label without storing a second spelling.
    ///
    /// Occurrence-oriented features must use that occurrence's source or edition instead.
    pub fn declaration_site_name<'display>(
        &'display self,
        declaration: &'display Declaration,
    ) -> anyhow::Result<DeclarationDisplayName<'display>> {
        let syntax = SyntaxRenderer::new(self.db.crate_edition(declaration.crate_ref)?);
        Ok(match &declaration.label {
            DeclarationLabel::Name(name) => DeclarationDisplayName::Name(syntax.name(name)),
            DeclarationLabel::TupleField(index) => DeclarationDisplayName::TupleField(*index),
            DeclarationLabel::Unsupported => DeclarationDisplayName::Unsupported,
            DeclarationLabel::Impl(item) => {
                let view = ItemStoreQuery::new(self.db)
                    .semantic_item_view(*item)?
                    .context("impl declaration item is unavailable")?;
                let (self_ty, trait_ref) = view
                    .impl_header()
                    .context("impl declaration has no impl header")?;
                DeclarationDisplayName::Impl {
                    syntax,
                    self_ty,
                    trait_ref,
                }
            }
        })
    }

    /// Return the file backing a root module.
    pub fn root_module_file(&self, module_ref: ModuleRef) -> anyhow::Result<Option<FileId>> {
        let Some(module) = self.db.module_data(module_ref)? else {
            return Ok(None);
        };
        let ModuleOrigin::Root { file_id } = module.origin else {
            return Ok(None);
        };
        Ok(Some(file_id))
    }

    /// Return declaration facts for an inline or out-of-line module declaration.
    fn module(&self, module_ref: ModuleRef) -> anyhow::Result<Option<Declaration>> {
        let Some(module) = self.db.module_data(module_ref)? else {
            return Ok(None);
        };
        let Some(name) = module.name.clone() else {
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

        Ok(Some(Declaration::named(
            module_ref.origin.origin_crate(),
            SymbolKind::Module,
            name,
            file_id,
            span,
            module.name_span.unwrap_or(span),
        )))
    }

    /// Return declaration facts for a DefMap local item.
    fn local_def(&self, local_def: LocalDefRef) -> anyhow::Result<Option<Declaration>> {
        let Some(data) = self.db.local_def_data(local_def)? else {
            return Ok(None);
        };

        Ok(Some(Declaration::named(
            local_def.origin.origin_crate(),
            SymbolKind::from_local_def_kind(data.kind),
            data.name.clone(),
            data.file_id,
            data.span,
            data.name_span.unwrap_or(data.span),
        )))
    }

    /// Return declaration facts for a semantic item.
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
                let Some(local_impl) = self.db.local_impl_data(local_impl_ref)? else {
                    return Ok(None);
                };
                Ok(Some(Declaration {
                    crate_ref: item.origin().origin_crate(),
                    kind: SymbolKind::Impl,
                    label: DeclarationLabel::Impl(item),
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
                let Some(name) = view.name().cloned() else {
                    return Ok(None);
                };
                let Some(span) = view.span() else {
                    return Ok(None);
                };

                Ok(Some(Declaration::named(
                    item.origin().origin_crate(),
                    SymbolKind::from_semantic_item_kind(view.kind()),
                    name,
                    view.source().file_id,
                    span,
                    view.name_span().unwrap_or(span),
                )))
            }
        }
    }

    /// Return declaration facts for a body binding.
    fn body_binding(&self, binding_ref: BodyBindingRef) -> anyhow::Result<Option<Declaration>> {
        let Some(body) = self.db.body_ir.body_data(binding_ref.body)? else {
            return Ok(None);
        };
        let Some(binding) = body.binding(binding_ref.binding) else {
            return Ok(None);
        };

        let selection_span = binding.name_span.unwrap_or(binding.source.span);
        Ok(Some(match &binding.name {
            Some(name) => Declaration::named(
                binding_ref.body.crate_ref,
                SymbolKind::Variable,
                name.clone(),
                binding.source.file_id,
                binding.source.span,
                selection_span,
            ),
            None => Declaration {
                crate_ref: binding_ref.body.crate_ref,
                kind: SymbolKind::Variable,
                label: DeclarationLabel::Unsupported,
                file_id: binding.source.file_id,
                span: binding.source.span,
                selection_span,
            },
        }))
    }

    /// Return declaration facts for an enum variant.
    fn semantic_enum_variant(
        &self,
        variant_ref: EnumVariantRef,
    ) -> anyhow::Result<Option<Declaration>> {
        let Some(data) = ItemStoreQuery::new(self.db).enum_variant_data(variant_ref)? else {
            return Ok(None);
        };

        Ok(Some(Declaration::named(
            variant_ref.origin.origin_crate(),
            SymbolKind::EnumVariant,
            data.variant.name.clone(),
            data.file_id,
            data.variant.span,
            data.variant.name_span,
        )))
    }

    /// Return declaration facts for a declared field.
    fn semantic_field(&self, field: FieldRef) -> anyhow::Result<Option<Declaration>> {
        let Some(data) = ItemStoreQuery::new(self.db).field_data(field)? else {
            return Ok(None);
        };
        let Some(key) = data.field.key.as_ref() else {
            return Ok(None);
        };

        let crate_ref = field.owner.origin.origin_crate();
        Ok(Some(match key {
            FieldKey::Named(name) => Declaration::named(
                crate_ref,
                SymbolKind::Field,
                name.clone(),
                data.file_id,
                data.field.span,
                data.field.span,
            ),
            FieldKey::Tuple(index) => Declaration::tuple_field(
                crate_ref,
                SymbolKind::Field,
                *index,
                data.file_id,
                data.field.span,
                data.field.span,
            ),
        }))
    }

    /// Return declaration facts for a function or method.
    fn semantic_function(&self, function: FunctionRef) -> anyhow::Result<Option<Declaration>> {
        let Some(data) = ItemStoreQuery::new(self.db).function_data(function)? else {
            return Ok(None);
        };

        Ok(Some(Declaration::named(
            function.origin.origin_crate(),
            match data.owner {
                ItemOwner::Module(_) => SymbolKind::Function,
                ItemOwner::Trait(_) | ItemOwner::Impl(_) => SymbolKind::Method,
            },
            data.name.clone(),
            data.source.file_id,
            data.span,
            data.name_span.unwrap_or(data.span),
        )))
    }
}
