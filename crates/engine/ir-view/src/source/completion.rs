//! Completion-site source facts.
//!
//! This adapter scans domain-owned source facts and exposes only the normalized sites that
//! completion needs to choose candidates and replacement spans.
//! It locates the incomplete syntax; candidate lookup and semantic filtering happen in analysis.
//!
//! ```text
//! value.na$0       -> receiver expression + replacement span `na`
//! model::Us$0      -> qualifier `model` + type/value namespace from context
//! fn load(_: Us$0) -> item signature scope + replacement span `Us`
//! User { na$0 }    -> record owner + already-written fields
//! ```

use anyhow::Context as _;
use rg_ir_model::{
    CrateRef, FieldKey, GenericDefRef, ModuleRef, Path,
    identity::{ExprRef, LexicalScopeRef},
};
use rg_parse::{FileId, Span};
use rg_semantic_ir::TypePathContext;

use super::scan::{
    BodyUnqualifiedNameContext, DotCompletionSiteScanner, ImportPathCompletionSiteScanner,
    PathCompletionSiteScanner, RecordFieldCompletionSiteScanner, SignatureCompletionSite,
    SignatureSourceScanner, SignatureTypePathScope, TypeNamePosition,
    UnqualifiedCompletionSiteScanner,
};
use crate::{IndexedViewDb, lookup::name::ValueOrTypeNamespace};

/// Source site for member access after a dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexedMemberAccessSite {
    receiver: ExprRef,
    member_prefix_span: Span,
}

impl IndexedMemberAccessSite {
    pub fn receiver(self) -> ExprRef {
        self.receiver
    }

    pub fn member_prefix_span(self) -> Span {
        self.member_prefix_span
    }
}

/// Source site for a qualified path segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedQualifiedPathSite {
    scope: IndexedQualifiedPathScope,
    qualifier: Path,
    member_prefix_span: Span,
}

impl IndexedQualifiedPathSite {
    pub fn scope(&self) -> IndexedQualifiedPathScope {
        self.scope
    }

    pub fn qualifier(&self) -> &Path {
        &self.qualifier
    }

    pub fn member_prefix_span(&self) -> Span {
        self.member_prefix_span
    }
}

/// Resolution context for a qualified path source site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedQualifiedPathScope {
    /// A path in an expression or body-owned type, such as `let _: model::Us$0`.
    Body {
        scope: LexicalScopeRef,
        namespace: ValueOrTypeNamespace,
    },
    /// A path in an import, such as `use model::Us$0;`.
    Import { module: ModuleRef },
    /// A type path in an item declaration, such as `fn load(_: model::Us$0)`.
    Signature { scope: IndexedSignatureTypeScope },
}

/// Semantic owner of a type path written in an item signature.
///
/// The type-path context resolves module names and impl `Self`; the generic owner identifies the
/// type and const parameters inherited by this particular declaration. For example, the cursor in
/// `impl<T> Wrapper<T> { fn map<U>(_: U$0) {} }` needs the function owner to see `U`, while its
/// type-path context supplies the impl's module and `Self` type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexedSignatureTypeScope {
    context: TypePathContext,
    generic_owner: GenericDefRef,
}

impl IndexedSignatureTypeScope {
    pub fn context(self) -> TypePathContext {
        self.context
    }

    pub fn generic_owner(self) -> GenericDefRef {
        self.generic_owner
    }
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
}

/// Resolution context for an unqualified name source site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexedUnqualifiedNameScope {
    /// A body name with its lexical cutoff, such as `inp$0` after several local bindings.
    Body {
        scope: LexicalScopeRef,
        context: IndexedUnqualifiedNameContext,
        member_prefix: String,
        visible_bindings: usize,
    },
    /// A declaration type name whose owner contributes generic parameters, such as `T$0` here:
    /// `fn load<T>(_: T$0)`.
    Signature {
        scope: IndexedSignatureTypeScope,
        context: IndexedUnqualifiedNameContext,
        member_prefix: String,
    },
    /// An import-root name resolved from the containing module, such as `use st$0;`.
    Import { module: ModuleRef },
}

/// Signature completion site normalized into the same path/name shapes as body completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexedSignatureTypeSite {
    /// A type path such as `fn load(_: model::Us$0)`.
    Qualified(IndexedQualifiedPathSite),
    /// A type name such as `fn load<T>(_: T$0)`.
    Unqualified(IndexedUnqualifiedNameSite),
}

/// Source site for record literal or pattern field names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedRecordFieldListSite {
    scope: LexicalScopeRef,
    owner: Path,
    member_prefix_span: Span,
    existing_fields: Vec<FieldKey>,
}

impl IndexedRecordFieldListSite {
    pub fn scope(&self) -> LexicalScopeRef {
        self.scope
    }

    pub fn owner(&self) -> &Path {
        &self.owner
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

    /// Return the member-access site at a cursor offset, e.g. `items.pu$0`.
    pub fn member_access_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedMemberAccessSite>> {
        Ok(
            DotCompletionSiteScanner::new(&self.db.body_ir, crate_ref, file_id, offset)
                .site_at_dot()?
                .map(|site| IndexedMemberAccessSite {
                    receiver: ExprRef::new(site.body, site.receiver),
                    member_prefix_span: site.member_prefix_span,
                }),
        )
    }

    /// Return a body qualified-path site at a cursor offset, e.g. `model::Us$0` in an expression.
    pub fn body_qualified_path_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedQualifiedPathSite>> {
        Ok(
            PathCompletionSiteScanner::new(&self.db.body_ir, crate_ref, file_id, offset)
                .site_at_path()?
                .map(|site| IndexedQualifiedPathSite {
                    scope: IndexedQualifiedPathScope::Body {
                        scope: LexicalScopeRef::new(site.body, site.scope),
                        namespace: site.namespace,
                    },
                    qualifier: site.qualifier,
                    member_prefix_span: site.member_prefix_span,
                }),
        )
    }

    /// Return an import qualified-path site at a cursor offset, e.g. `use model::Us$0;`.
    pub fn import_qualified_path_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedQualifiedPathSite>> {
        Ok(
            ImportPathCompletionSiteScanner::new(&self.db.def_map, crate_ref, file_id, offset)
                .qualified_site()?
                .map(|site| IndexedQualifiedPathSite {
                    scope: IndexedQualifiedPathScope::Import {
                        module: site.module,
                    },
                    qualifier: site.qualifier,
                    member_prefix_span: site.member_prefix_span,
                }),
        )
    }

    /// Return a body unqualified-name site at a cursor offset, e.g. `let value = inp$0;`.
    pub fn body_unqualified_name_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        Ok(
            UnqualifiedCompletionSiteScanner::new(&self.db.body_ir, crate_ref, file_id, offset)
                .site_at_name()?
                .map(|site| IndexedUnqualifiedNameSite {
                    scope: IndexedUnqualifiedNameScope::Body {
                        scope: LexicalScopeRef::new(site.body, site.scope),
                        context: match site.context {
                            BodyUnqualifiedNameContext::Type(position) => {
                                IndexedUnqualifiedNameContext::Type {
                                    position: Self::type_name_position(position),
                                }
                            }
                            BodyUnqualifiedNameContext::Value => {
                                IndexedUnqualifiedNameContext::Value
                            }
                        },
                        member_prefix: site.member_prefix,
                        visible_bindings: site.visible_bindings,
                    },
                    member_prefix_span: site.member_prefix_span,
                }),
        )
    }

    /// Return a type-path completion site from an item signature.
    ///
    /// This covers declaration-owned syntax such as function parameters, return types, fields,
    /// generic bounds, impl headers, and aliases. The result deliberately reuses the same
    /// qualified/unqualified vocabulary as body completion so candidate rendering has one path.
    ///
    /// ```text
    /// fn load(_: model::Us$0) -> qualified path, qualifier `model`, prefix `Us`
    /// fn load<T>(_: T$0)      -> unqualified name, generic owner `load`, prefix `T`
    /// ```
    pub fn signature_type_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedSignatureTypeSite>> {
        let site = SignatureSourceScanner::completion_site_at(
            &self.db.semantic_ir,
            crate_ref,
            file_id,
            offset,
        )
        .context("scan signature type completion site")?;

        Ok(site.map(|site| match site {
            SignatureCompletionSite::Qualified {
                scope,
                qualifier,
                member_prefix_span,
            } => IndexedSignatureTypeSite::Qualified(IndexedQualifiedPathSite {
                scope: IndexedQualifiedPathScope::Signature {
                    scope: Self::signature_scope(scope),
                },
                qualifier,
                member_prefix_span,
            }),
            SignatureCompletionSite::Unqualified {
                scope,
                member_prefix_span,
                member_prefix,
                position,
            } => IndexedSignatureTypeSite::Unqualified(IndexedUnqualifiedNameSite {
                scope: IndexedUnqualifiedNameScope::Signature {
                    scope: Self::signature_scope(scope),
                    context: IndexedUnqualifiedNameContext::Type {
                        position: Self::type_name_position(position),
                    },
                    member_prefix,
                },
                member_prefix_span,
            }),
        }))
    }

    /// Return an import unqualified-name site at a cursor offset, e.g. `use cr$0;`.
    pub fn import_unqualified_name_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        Ok(
            ImportPathCompletionSiteScanner::new(&self.db.def_map, crate_ref, file_id, offset)
                .unqualified_site()?
                .map(|site| IndexedUnqualifiedNameSite {
                    scope: IndexedUnqualifiedNameScope::Import {
                        module: site.module,
                    },
                    member_prefix_span: site.member_prefix_span,
                }),
        )
    }

    /// Return a record field-list site at a cursor offset, e.g. `User { na$0 }`.
    pub fn record_field_list_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedRecordFieldListSite>> {
        Ok(
            RecordFieldCompletionSiteScanner::new(&self.db.body_ir, crate_ref, file_id, offset)
                .site_at_record_field()?
                .map(|site| IndexedRecordFieldListSite {
                    scope: LexicalScopeRef::new(site.body, site.scope),
                    owner: site.owner,
                    member_prefix_span: site.member_prefix_span,
                    existing_fields: site.existing_fields,
                }),
        )
    }

    fn type_name_position(position: TypeNamePosition) -> IndexedTypeNamePosition {
        match position {
            TypeNamePosition::Type => IndexedTypeNamePosition::Type,
            TypeNamePosition::BareGenericArgument => IndexedTypeNamePosition::BareGenericArgument,
        }
    }

    fn signature_scope(scope: SignatureTypePathScope) -> IndexedSignatureTypeScope {
        IndexedSignatureTypeScope {
            context: scope.context,
            generic_owner: scope.generic_owner,
        }
    }
}
