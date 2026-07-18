//! Completion-site source facts.
//!
//! This adapter scans domain-owned source facts and exposes only the normalized sites that
//! completion needs to choose candidates and replacement spans.
//! It locates the incomplete syntax; candidate lookup and semantic filtering happen in analysis.
//!
//! ```text
//! value.na$0       -> receiver expression + replacement span `na`
//! model::Us$0      -> qualifier `model` + type/value namespace from context
//! User { na$0 }    -> record owner + already-written fields
//! ```

use rg_ir_model::Path;
use rg_ir_model::items::FieldKey;
use rg_ir_model::{
    CrateRef, ModuleRef,
    identity::{ExprRef, LexicalScopeRef},
};
use rg_parse::{FileId, Span};

use super::scan::{
    DotCompletionSiteScanner, ImportPathCompletionSiteScanner, PathCompletionNamespace,
    PathCompletionSiteScanner, RecordFieldCompletionSiteScanner, UnqualifiedCompletionNamespace,
    UnqualifiedCompletionSiteScanner,
};
use crate::IndexedViewDb;

/// Namespace expected by an indexed name site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedNameNamespace {
    Types,
    Values,
}

impl From<PathCompletionNamespace> for IndexedNameNamespace {
    fn from(namespace: PathCompletionNamespace) -> Self {
        match namespace {
            PathCompletionNamespace::Types => Self::Types,
            PathCompletionNamespace::Values => Self::Values,
        }
    }
}

impl From<UnqualifiedCompletionNamespace> for IndexedNameNamespace {
    fn from(namespace: UnqualifiedCompletionNamespace) -> Self {
        match namespace {
            UnqualifiedCompletionNamespace::Types => Self::Types,
            UnqualifiedCompletionNamespace::Values => Self::Values,
        }
    }
}

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
    Body {
        scope: LexicalScopeRef,
        namespace: IndexedNameNamespace,
    },
    Import {
        module: ModuleRef,
    },
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
    Body {
        scope: LexicalScopeRef,
        namespace: IndexedNameNamespace,
        member_prefix: String,
        visible_bindings: usize,
    },
    Import {
        module: ModuleRef,
    },
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
                        namespace: site.namespace.into(),
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
                        namespace: site.namespace.into(),
                        member_prefix: site.member_prefix,
                        visible_bindings: site.visible_bindings,
                    },
                    member_prefix_span: site.member_prefix_span,
                }),
        )
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
}
