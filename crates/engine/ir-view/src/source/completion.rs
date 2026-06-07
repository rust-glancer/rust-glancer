//! Completion-site source facts.
//!
//! This adapter only normalizes scanner-specific cursor sites into the shapes completion uses to
//! choose candidates and replacement spans.

use rg_body_ir::{
    PathCompletionNamespace as BodyPathCompletionNamespace,
    UnqualifiedCompletionNamespace as BodyUnqualifiedCompletionNamespace,
};
use rg_ir_model::Path;
use rg_ir_model::items::FieldKey;
use rg_ir_model::{
    ModuleRef, TargetRef,
    identity::{ExprRef, LexicalScopeRef},
};
use rg_parse::{FileId, Span};

use crate::IndexedViewDb;

/// Namespace expected by an indexed name site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedNameNamespace {
    Types,
    Values,
}

impl From<BodyPathCompletionNamespace> for IndexedNameNamespace {
    fn from(namespace: BodyPathCompletionNamespace) -> Self {
        match namespace {
            BodyPathCompletionNamespace::Types => Self::Types,
            BodyPathCompletionNamespace::Values => Self::Values,
        }
    }
}

impl From<BodyUnqualifiedCompletionNamespace> for IndexedNameNamespace {
    fn from(namespace: BodyUnqualifiedCompletionNamespace) -> Self {
        match namespace {
            BodyUnqualifiedCompletionNamespace::Types => Self::Types,
            BodyUnqualifiedCompletionNamespace::Values => Self::Values,
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

pub struct SourceCompletionView<'a, 'db> {
    analysis: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> SourceCompletionView<'a, 'db> {
    pub fn new(analysis: &'a IndexedViewDb<'db>) -> Self {
        Self { analysis }
    }

    pub fn member_access_site_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedMemberAccessSite>> {
        Ok(self
            .analysis
            .body_ir
            .dot_completion_site(target, file_id, offset)?
            .map(|site| IndexedMemberAccessSite {
                receiver: ExprRef::new(site.body, site.receiver),
                member_prefix_span: site.member_prefix_span,
            }))
    }

    pub fn body_qualified_path_site_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedQualifiedPathSite>> {
        Ok(self
            .analysis
            .body_ir
            .path_completion_site(target, file_id, offset)?
            .map(|site| IndexedQualifiedPathSite {
                scope: IndexedQualifiedPathScope::Body {
                    scope: LexicalScopeRef::new(site.body, site.scope),
                    namespace: site.namespace.into(),
                },
                qualifier: site.qualifier,
                member_prefix_span: site.member_prefix_span,
            }))
    }

    pub fn import_qualified_path_site_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedQualifiedPathSite>> {
        Ok(self
            .analysis
            .def_map
            .path_completion_site(target, file_id, offset)?
            .map(|site| IndexedQualifiedPathSite {
                scope: IndexedQualifiedPathScope::Import {
                    module: site.module,
                },
                qualifier: site.qualifier,
                member_prefix_span: site.member_prefix_span,
            }))
    }

    pub fn body_unqualified_name_site_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        Ok(self
            .analysis
            .body_ir
            .unqualified_completion_site(target, file_id, offset)?
            .map(|site| IndexedUnqualifiedNameSite {
                scope: IndexedUnqualifiedNameScope::Body {
                    scope: LexicalScopeRef::new(site.body, site.scope),
                    namespace: site.namespace.into(),
                    member_prefix: site.member_prefix,
                    visible_bindings: site.visible_bindings,
                },
                member_prefix_span: site.member_prefix_span,
            }))
    }

    pub fn import_unqualified_name_site_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        Ok(self
            .analysis
            .def_map
            .unqualified_completion_site(target, file_id, offset)?
            .map(|site| IndexedUnqualifiedNameSite {
                scope: IndexedUnqualifiedNameScope::Import {
                    module: site.module,
                },
                member_prefix_span: site.member_prefix_span,
            }))
    }

    pub fn record_field_list_site_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedRecordFieldListSite>> {
        Ok(self
            .analysis
            .body_ir
            .record_field_completion_site(target, file_id, offset)?
            .map(|site| IndexedRecordFieldListSite {
                scope: LexicalScopeRef::new(site.body, site.scope),
                owner: site.owner,
                member_prefix_span: site.member_prefix_span,
                existing_fields: site.existing_fields,
            }))
    }
}
