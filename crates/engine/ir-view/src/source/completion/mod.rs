//! Completion source sites normalized across indexed and request-local syntax.
//!
//! Complete-enough syntax is scanned from DefMap, Semantic IR, and Body IR. Actively typed forms
//! that lowering cannot retain—an empty path, a trailing `::`, or an empty record field
//! list—arrive from request-local syntax and are attached here to the nearest indexed module,
//! body, or signature scope.
//!
//! The result describes source ownership rather than final completion policy: receiver identity,
//! module or impl ownership, visible labels, path interpretations, lexical or generic scope,
//! replacement span, and names already written at the site. Name and member views discover
//! semantic candidates; analysis decides filtering, ranking, and insertion text.
//!
//! ```text
//! value.na$0                    -> receiver expression + replacement span `na`
//! Widget::<u8>::ne$0            -> module and type-shaped qualifier interpretations
//! Iterator<It$0 = u8>           -> trait qualifier + binding replacement span
//! fn load<T>(_: $0)             -> signature scope where `T` is visible
//! User { name, na$0 }           -> resolved record owner + existing field `name`
//! break 'in$0                    -> enclosing labels, nearest target first
//! mod na$0;                      -> semantic module + inline filesystem descent
//! impl Service for Worker { re$0 } -> resolved impl and trait identities
//! ```

mod body;
mod import;
mod signature;

use anyhow::Context as _;
use rg_ir_model::{
    BodyBindingRef, CrateRef, EnumVariantRef, FieldKey, GenericDefRef, ImplRef, ModuleRef, Path,
    TraitDefRef, TypeDefRef,
    identity::{ExprRef, LexicalScopeRef},
};
use rg_item_tree::{FromAst as _, TypePath, TypeRef};
use rg_parse::{FileId, LineIndex, Span};
use rg_syntax::{AstNode as _, Edition, SourceFile, ast};
use rg_text::NameInterner;

use super::occurrence::IndexedSignatureTypeScope;
use super::scan::{
    AssociatedPathQualifier, BodyQualifiedPathContext, ModuleFileBase, ModuleSourceSiteScanner,
    PathCompletionSiteScanner, PatternCompletionKind, SignatureCompletionSite,
    SignatureSourceScanner, SignatureTypePathScope, TraitImplSourceSiteScanner, TypeNamePosition,
};
use crate::IndexedViewDb;

/// Source site for member access after a dot.
///
/// The expression id supplies the inferred receiver type. Its exact written span is retained
/// separately so request-local postfix syntax can verify that it is describing the same receiver
/// before adding edits that replace the whole expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexedMemberAccessSite {
    receiver: ExprRef,
    scope: LexicalScopeRef,
    receiver_span: Span,
    member_prefix_span: Span,
}

/// Semantic module owning a module-scope source position, plus filesystem completion facts.
///
/// For a cursor in the declaration below, `module` is `v1` and `inline_module_path` is
/// `["api", "v1"]`:
///
/// ```text
/// mod api {
///     mod v1 {
///         mod na$0;
///     }
/// }
/// ```
///
/// The file base distinguishes target roots, conventional module files, and `#[path]` files. The
/// inline path then tells filesystem lookup which directories to descend below that base.
/// `declared_children` lets module-name completion exclude siblings that already have a `mod`
/// declaration; the declaration under the cursor is deliberately not included in that set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedModuleSourceSite {
    module: ModuleRef,
    file_base: IndexedModuleFileBase,
    inline_module_path: Vec<String>,
    declared_children: Vec<String>,
}

/// Filesystem lookup rule inherited from the nearest file-backed semantic module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedModuleFileBase {
    TargetRoot,
    Conventional,
    PathAttribute,
}

impl From<ModuleFileBase> for IndexedModuleFileBase {
    fn from(file_base: ModuleFileBase) -> Self {
        match file_base {
            ModuleFileBase::TargetRoot => Self::TargetRoot,
            ModuleFileBase::Conventional => Self::Conventional,
            ModuleFileBase::PathAttribute => Self::PathAttribute,
        }
    }
}

impl IndexedModuleSourceSite {
    pub fn module(&self) -> ModuleRef {
        self.module
    }

    pub fn file_base(&self) -> IndexedModuleFileBase {
        self.file_base
    }

    pub fn inline_module_path(&self) -> &[String] {
        &self.inline_module_path
    }

    pub fn declared_children(&self) -> &[String] {
        &self.declared_children
    }
}

/// Resolved trait implementation owning an associated-item list.
///
/// The impl identity supplies the concrete header and trait substitution. The trait identity
/// supplies the canonical member list whose missing declarations can be projected into that impl.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexedTraitImplSite {
    impl_ref: ImplRef,
    trait_ref: TraitDefRef,
}

impl IndexedTraitImplSite {
    pub fn impl_ref(self) -> ImplRef {
        self.impl_ref
    }

    pub fn trait_ref(self) -> TraitDefRef {
        self.trait_ref
    }
}

impl IndexedMemberAccessSite {
    pub fn receiver(self) -> ExprRef {
        self.receiver
    }

    pub fn scope(self) -> LexicalScopeRef {
        self.scope
    }

    pub fn receiver_span(self) -> Span {
        self.receiver_span
    }

    pub fn member_prefix_span(self) -> Span {
        self.member_prefix_span
    }
}

/// Source site for a qualified path segment and the lookup interpretations its prefix supports.
///
/// A spelling such as `model::Widget::ne$0` may interpret `model::Widget` as either a module path
/// or a type-shaped qualifier. Keeping both projections lets candidate lookup try each
/// interpretation without reparsing source or flattening generic arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedQualifiedPathSite {
    scope: IndexedQualifiedPathScope,
    module_qualifier: Option<Path>,
    associated_qualifier: Option<IndexedAssociatedPathQualifier>,
    member_prefix_span: Span,
}

impl IndexedQualifiedPathSite {
    pub fn scope(&self) -> IndexedQualifiedPathScope {
        self.scope
    }

    pub fn module_qualifier(&self) -> Option<&Path> {
        self.module_qualifier.as_ref()
    }

    pub fn associated_qualifier(&self) -> Option<&IndexedAssociatedPathQualifier> {
        self.associated_qualifier.as_ref()
    }

    pub fn member_prefix_span(&self) -> Span {
        self.member_prefix_span
    }

    /// Keep saved semantic scope while taking the editable prefix range from current syntax.
    pub fn with_current_member_prefix(mut self, member_prefix_span: Span) -> Self {
        self.member_prefix_span = member_prefix_span;
        self
    }
}

/// Type-shaped path prefix retained for associated-item lookup.
///
/// This preserves syntax that a DefMap `Path` cannot represent, notably generic arguments and
/// qualified anchors:
///
/// ```text
/// Widget::<u8>::ne$0 -> Type(Widget::<u8>)
/// <T as Factory>::ne$0 -> QualifiedTrait { self_ty: T, trait_ref: Factory }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexedAssociatedPathQualifier {
    /// An ordinary type-shaped prefix such as `Widget::<u8>` or `T`.
    Type(TypeRef),
    /// The two sides of an explicitly selected trait prefix such as `<T as Factory>`.
    QualifiedTrait {
        self_ty: TypeRef,
        trait_ref: TypeRef,
    },
}

/// Resolution context for a qualified path source site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedQualifiedPathScope {
    /// A path in an expression or body-owned type, such as `let _: model::Us$0`.
    Body {
        scope: LexicalScopeRef,
        context: IndexedQualifiedPathContext,
    },
    /// A path in an import, such as `use model::Us$0;`.
    Import { module: ModuleRef },
    /// A type path in an item declaration, such as `fn load(_: model::Us$0)`.
    Signature { scope: IndexedSignatureTypeScope },
}

/// Completion rules selected by syntax around a body-qualified path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedQualifiedPathContext {
    /// A path naming a type, such as `let _: model::Us$0`.
    Type,
    /// A path naming a value or callable, such as `model::ma$0()`.
    Value,
    /// A value path inside a type-level or declaration-level const expression.
    Const,
    /// A constructor path whose following syntax constrains its insertion shape.
    Pattern(IndexedPatternCompletionKind),
}

/// Pattern surface that constrains valid path candidates and insertion shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedPatternCompletionKind {
    /// A bare pattern name such as `Sta$0`.
    Name,
    /// A constructor followed by tuple fields, such as `Act$0(value)`.
    TupleConstructor,
    /// A constructor followed by named fields, such as `Act$0 { field }`.
    RecordConstructor,
}

/// Position of an unqualified type-shaped name within its surrounding annotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedTypeNamePosition {
    /// A path in ordinary type syntax, including structured types nested in generic arguments.
    Type,
    /// A whole `N` argument in syntax such as `Array<N>`, which may name a const parameter.
    BareGenericArgument,
}

/// Namespace and generic-argument context selected by unqualified source syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedUnqualifiedNameContext {
    /// A type-shaped name such as `Us$0` in `fn load(_: Us$0)`.
    Type { position: IndexedTypeNamePosition },
    /// A value-shaped name such as `inp$0` in `let value = inp$0`.
    Value,
    /// A value path in a type-level or declaration-level const expression.
    Const,
    /// A pattern-shaped name such as `Sta$0` in a match arm.
    Pattern(IndexedPatternCompletionKind),
}

/// Source site for an unqualified name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedUnqualifiedNameSite {
    scope: IndexedUnqualifiedNameScope,
    member_prefix_span: Span,
}

impl IndexedUnqualifiedNameSite {
    pub fn scope(&self) -> &IndexedUnqualifiedNameScope {
        &self.scope
    }

    pub fn member_prefix_span(&self) -> Span {
        self.member_prefix_span
    }

    pub fn member_prefix(&self) -> &str {
        match &self.scope {
            IndexedUnqualifiedNameScope::Body { member_prefix, .. }
            | IndexedUnqualifiedNameScope::Signature { member_prefix, .. }
            | IndexedUnqualifiedNameScope::Module { member_prefix, .. }
            | IndexedUnqualifiedNameScope::Import { member_prefix, .. } => member_prefix,
        }
    }

    /// Keep saved semantic scope while taking the written prefix and edit range from current
    /// syntax.
    pub fn with_current_member_prefix(
        mut self,
        member_prefix_span: Span,
        member_prefix: String,
    ) -> Self {
        self.member_prefix_span = member_prefix_span;
        match &mut self.scope {
            IndexedUnqualifiedNameScope::Body {
                member_prefix: current,
                ..
            }
            | IndexedUnqualifiedNameScope::Signature {
                member_prefix: current,
                ..
            }
            | IndexedUnqualifiedNameScope::Module {
                member_prefix: current,
                ..
            }
            | IndexedUnqualifiedNameScope::Import {
                member_prefix: current,
                ..
            } => *current = member_prefix,
        }
        self
    }
}

/// Resolution context for an unqualified name source site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexedUnqualifiedNameScope {
    /// A body name with its lexical cutoff, such as `inp$0` after several local bindings.
    Body {
        scope: LexicalScopeRef,
        context: IndexedUnqualifiedNameContext,
        member_prefix: String,
        /// Body-local declaration whose generics own the source, if this is its signature.
        generic_owner: Option<GenericDefRef>,
        /// Ambiguous pattern binding whose inferred type can provide expected enum variants.
        expected_type_binding: Option<BodyBindingRef>,
        /// Source-order boundary that prevents later body bindings from entering the scope.
        visible_bindings: usize,
    },
    /// A declaration type name whose owner contributes generic parameters, such as `T$0` here:
    /// `fn load<T>(_: T$0)`.
    Signature {
        scope: IndexedSignatureTypeScope,
        context: IndexedUnqualifiedNameContext,
        member_prefix: String,
    },
    /// A name in current module-level syntax resolved against the matching saved module.
    Module {
        module: ModuleRef,
        context: IndexedUnqualifiedNameContext,
        member_prefix: String,
    },
    /// An import-root name resolved from the containing module, such as `use st$0;`.
    Import {
        module: ModuleRef,
        member_prefix: String,
    },
}

/// Signature completion site normalized into the same path/name shapes as body completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexedSignatureTypeSite {
    /// A trait binding already disambiguated by `=`, such as `Iterator<It$0 = u8>`.
    AssociatedTypeBinding(IndexedAssociatedTypeBindingSite),
    /// A type path such as `fn load(_: model::Us$0)`.
    Qualified(IndexedQualifiedPathSite),
    /// A type name such as `fn load<T>(_: T$0)`.
    Unqualified(IndexedUnqualifiedNameSite),
}

impl IndexedSignatureTypeSite {
    /// Keep the associated saved signature scope, but make every editor-facing range and prefix
    /// belong to current syntax.
    pub fn with_current_member_prefix(
        self,
        member_prefix_span: Span,
        member_prefix: String,
    ) -> Self {
        match self {
            Self::AssociatedTypeBinding(site) => {
                Self::AssociatedTypeBinding(site.with_current_member_prefix(member_prefix_span))
            }
            Self::Qualified(site) => {
                Self::Qualified(site.with_current_member_prefix(member_prefix_span))
            }
            Self::Unqualified(site) => Self::Unqualified(
                site.with_current_member_prefix(member_prefix_span, member_prefix),
            ),
        }
    }
}

/// Normalized source facts for an associated type binding name.
///
/// In `Iterator<It$0 = u8>`, `trait_ref` is the surrounding `Iterator` use with associated
/// bindings removed, `member_prefix_span` selects `It`, and `existing_bindings` contains the other
/// binding names already written on that trait use. The scope says whether `Iterator` must be
/// resolved from a body or a declaration signature.
///
/// The same value also represents the speculative pre-`=` form `Iterator<It$0>`. That distinction
/// belongs to the scanning method: an explicit site replaces normal type completion, while an
/// implicit site is only an overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedAssociatedTypeBindingSite {
    scope: IndexedAssociatedTypeBindingScope,
    trait_ref: TypeRef,
    member_prefix_span: Span,
    existing_bindings: Vec<String>,
}

impl IndexedAssociatedTypeBindingSite {
    pub fn scope(&self) -> IndexedAssociatedTypeBindingScope {
        self.scope
    }

    pub fn trait_ref(&self) -> &TypeRef {
        &self.trait_ref
    }

    pub fn member_prefix_span(&self) -> Span {
        self.member_prefix_span
    }

    pub fn existing_bindings(&self) -> &[String] {
        &self.existing_bindings
    }

    /// Keep the saved trait and binding set while taking the editable range from current syntax.
    pub fn with_current_member_prefix(mut self, member_prefix_span: Span) -> Self {
        self.member_prefix_span = member_prefix_span;
        self
    }
}

/// Semantic owner used to resolve the trait named by an associated binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedAssociatedTypeBindingScope {
    /// Resolve the trait from a body lexical scope.
    Body { scope: LexicalScopeRef },
    /// Resolve the trait from an item signature and its generic owner.
    Signature { scope: IndexedSignatureTypeScope },
}

/// Source site for record literal or pattern field names.
///
/// The owner is resolved before this value crosses the view boundary. Existing keys are retained
/// so `User { name, na$0 }` does not offer `name` again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedRecordFieldListSite {
    scope: LexicalScopeRef,
    owner: IndexedRecordOwner,
    member_prefix_span: Span,
    existing_fields: Vec<FieldKey>,
}

/// Resolved declaration that owns a record field list.
///
/// A record path can name either a struct-like type (`User { ... }`) or one selected enum variant
/// (`Action::Stop { ... }`). Keeping that distinction here prevents candidate lookup from trying
/// to reinterpret a variant as a type path later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedRecordOwner {
    Type(TypeDefRef),
    EnumVariant(EnumVariantRef),
}

impl IndexedRecordFieldListSite {
    pub fn scope(&self) -> LexicalScopeRef {
        self.scope
    }

    pub fn owner(&self) -> IndexedRecordOwner {
        self.owner
    }

    pub fn member_prefix_span(&self) -> Span {
        self.member_prefix_span
    }

    pub fn existing_fields(&self) -> &[FieldKey] {
        &self.existing_fields
    }
}

/// Finds normalized completion sites by interpreting indexed domain facts.
///
/// Each method answers one syntactic completion family. Callers can try the relevant families in
/// editor-policy order without depending on DefMap, Semantic IR, or Body IR scanner types.
pub struct SourceCompletionView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> SourceCompletionView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    /// Return the semantic module and filesystem descent owning a module-scope cursor.
    pub fn module_source_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedModuleSourceSite>> {
        Ok(
            ModuleSourceSiteScanner::new(&self.db.def_map, crate_ref, file_id, offset)
                .site()
                .context("scan module completion source site")?
                .map(|site| IndexedModuleSourceSite {
                    module: site.module,
                    file_base: site.file_base.into(),
                    inline_module_path: site.inline_module_path,
                    declared_children: site.declared_children,
                }),
        )
    }

    /// Find the saved module named by the current inline-module path.
    ///
    /// An edit earlier in the file may make the saved byte range useless, while a path such as
    /// `outer::inner` still identifies the module. If the path is new, renamed, or ambiguous, there
    /// is no safe saved scope and this returns no completion site until save.
    pub fn module_syntax_name_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        inline_module_path: &[String],
        context: IndexedUnqualifiedNameContext,
        member_prefix_span: Span,
        member_prefix: String,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        Ok(ModuleSourceSiteScanner::module_for_inline_path(
            &self.db.def_map,
            crate_ref,
            file_id,
            inline_module_path,
        )
        .context("match current module syntax to saved module")?
        .map(|module| IndexedUnqualifiedNameSite {
            scope: IndexedUnqualifiedNameScope::Module {
                module,
                context,
                member_prefix,
            },
            member_prefix_span,
        }))
    }

    /// Resolve the semantic module named by current syntax for module-owned completion families.
    pub fn module_syntax_source_site(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        inline_module_path: &[String],
    ) -> anyhow::Result<Option<IndexedModuleSourceSite>> {
        Ok(ModuleSourceSiteScanner::site_for_inline_path(
            &self.db.def_map,
            crate_ref,
            file_id,
            inline_module_path,
        )
        .context("match current module syntax to saved module")?
        .map(|site| IndexedModuleSourceSite {
            module: site.module,
            file_base: site.file_base.into(),
            inline_module_path: site.inline_module_path,
            declared_children: site.declared_children,
        }))
    }

    /// Return the resolved trait implementation owning an associated-item-list cursor.
    pub fn trait_impl_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedTraitImplSite>> {
        Ok(
            TraitImplSourceSiteScanner::new(self.db, crate_ref, file_id, offset)
                .site()
                .context("scan trait impl completion source site")?
                .map(|site| IndexedTraitImplSite {
                    impl_ref: site.impl_ref,
                    trait_ref: site.trait_ref,
                }),
        )
    }

    /// Find a possible associated-type binding inside request-local Body IR.
    ///
    /// In `Iterator<It$0>`, `It` remains a valid ordinary type argument. This method returns an
    /// extra completion site for associated names; the caller must keep its ordinary type
    /// completions too. The offset belongs to the source that produced the Body IR.
    pub fn body_implicit_associated_type_binding_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedAssociatedTypeBindingSite>> {
        Ok(
            PathCompletionSiteScanner::new(&self.db.body_ir, crate_ref, file_id, offset)
                .implicit_associated_type_binding_site_at()
                .context("scan implicit body associated type binding site")?
                .map(|site| IndexedAssociatedTypeBindingSite {
                    scope: IndexedAssociatedTypeBindingScope::Body {
                        scope: LexicalScopeRef::new(site.body, site.scope),
                    },
                    trait_ref: site.trait_ref,
                    member_prefix_span: site.member_prefix_span,
                    existing_bindings: site.existing_bindings,
                }),
        )
    }

    /// Find a possible associated-type binding in the selected current declaration header.
    ///
    /// In `fn load(_: Iterator<It$0>) {}`, `It` can still be an ordinary type argument. This
    /// returns the additional associated-type interpretation with the declaration's request-local
    /// scope; the normal type-path site remains available beside it.
    pub fn current_signature_implicit_associated_type_binding_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedAssociatedTypeBindingSite>> {
        for origin in self.db.current_signature_origins(crate_ref, file_id)? {
            let Some(site) =
                SignatureSourceScanner::implicit_associated_type_binding_site_at_origin(
                    self.db, origin, file_id, offset,
                )
                .context("scan current signature associated type binding site")?
            else {
                continue;
            };
            let SignatureCompletionSite::AssociatedTypeBinding {
                scope,
                trait_ref,
                member_prefix_span,
                existing_bindings,
            } = site
            else {
                continue;
            };
            return Ok(Some(IndexedAssociatedTypeBindingSite {
                scope: IndexedAssociatedTypeBindingScope::Signature {
                    scope: Self::signature_scope(scope),
                },
                trait_ref,
                member_prefix_span,
                existing_bindings,
            }));
        }
        Ok(None)
    }

    /// Find a possible associated-type binding in a saved declaration signature.
    ///
    /// `offset` is a saved-source coordinate. Callers starting from editor text must first prove
    /// that the saved source is exact or map the cursor through a declaration association.
    pub fn signature_implicit_associated_type_binding_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedAssociatedTypeBindingSite>> {
        let Some(site) = SignatureSourceScanner::implicit_associated_type_binding_site_at(
            self.db, crate_ref, file_id, offset,
        )
        .context("scan implicit signature associated type binding site")?
        else {
            return Ok(None);
        };
        let SignatureCompletionSite::AssociatedTypeBinding {
            scope,
            trait_ref,
            member_prefix_span,
            existing_bindings,
        } = site
        else {
            return Ok(None);
        };
        Ok(Some(IndexedAssociatedTypeBindingSite {
            scope: IndexedAssociatedTypeBindingScope::Signature {
                scope: Self::signature_scope(scope),
            },
            trait_ref,
            member_prefix_span,
            existing_bindings,
        }))
    }

    /// Recover both lookup views of a request-local type-shaped qualifier.
    ///
    /// A cursor immediately after `Widget::<u8>::` has no final segment for the indexed source
    /// scanner to retain. Wrapping the qualifier in a synthetic alias gives the parser a complete
    /// type position:
    ///
    /// ```text
    /// type __RgCompletion = Widget::<u8>;
    /// ```
    ///
    /// The resulting `TypePath` preserves generic arguments and qualified-type anchors for
    /// associated-item lookup. Its optional DefMap projection supplies the ordinary module-path
    /// interpretation, because an ambiguous spelling such as `model::Widget` may need both.
    fn syntax_path_qualifiers(
        qualifier: &str,
    ) -> Option<(Option<Path>, IndexedAssociatedPathQualifier)> {
        let source = format!("type __RgCompletion = {qualifier};");
        let file = SourceFile::parse(&source, Edition::CURRENT).tree();
        let alias = file.syntax().descendants().find_map(ast::TypeAlias::cast)?;
        let ast::Type::PathType(path_type) = alias.ty()? else {
            return None;
        };
        let path = path_type.path()?;
        let line_index = LineIndex::new(&source);
        let mut interner = NameInterner::new();
        let path = TypePath::from_ast(&path, (&line_index, &mut interner));
        let module_qualifier = path.as_def_map_path();
        Some((
            module_qualifier,
            IndexedAssociatedPathQualifier::Type(TypeRef::Path(path)),
        ))
    }

    fn type_name_position(position: TypeNamePosition) -> IndexedTypeNamePosition {
        match position {
            TypeNamePosition::Type => IndexedTypeNamePosition::Type,
            TypeNamePosition::BareGenericArgument => IndexedTypeNamePosition::BareGenericArgument,
        }
    }

    fn associated_qualifier(qualifier: AssociatedPathQualifier) -> IndexedAssociatedPathQualifier {
        match qualifier {
            AssociatedPathQualifier::Type(ty) => IndexedAssociatedPathQualifier::Type(ty),
            AssociatedPathQualifier::QualifiedTrait { self_ty, trait_ref } => {
                IndexedAssociatedPathQualifier::QualifiedTrait { self_ty, trait_ref }
            }
        }
    }

    fn qualified_path_context(context: BodyQualifiedPathContext) -> IndexedQualifiedPathContext {
        match context {
            BodyQualifiedPathContext::Type => IndexedQualifiedPathContext::Type,
            BodyQualifiedPathContext::Value => IndexedQualifiedPathContext::Value,
            BodyQualifiedPathContext::Pattern(kind) => {
                IndexedQualifiedPathContext::Pattern(Self::pattern_kind(kind))
            }
        }
    }

    fn pattern_kind(kind: PatternCompletionKind) -> IndexedPatternCompletionKind {
        match kind {
            PatternCompletionKind::Name => IndexedPatternCompletionKind::Name,
            PatternCompletionKind::TupleConstructor => {
                IndexedPatternCompletionKind::TupleConstructor
            }
            PatternCompletionKind::RecordConstructor => {
                IndexedPatternCompletionKind::RecordConstructor
            }
        }
    }

    fn signature_scope(scope: SignatureTypePathScope) -> IndexedSignatureTypeScope {
        IndexedSignatureTypeScope::new(scope.context, scope.generic_owner)
    }
}
