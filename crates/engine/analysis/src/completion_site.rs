//! Completion-domain cursor sites.
//!
//! Source facts expose the syntax shapes that exist at a position. Completion owns the question
//! "which source site should this cursor use?", so this module wraps indexed sites behind a small
//! completion vocabulary.

use rg_ir_model::TargetRef;
use rg_parse::{FileId, Span};

use rg_ir_view::{
    IndexedViewDb,
    source::{
        IndexedMemberAccessSite, IndexedNameNamespace, IndexedQualifiedPathScope,
        IndexedQualifiedPathSite, IndexedRecordFieldListSite, IndexedUnqualifiedNameScope,
        IndexedUnqualifiedNameSite, SourceFactsView,
    },
};

pub(crate) enum CompletionSite {
    Dot(DotCompletionSite),
    Path(PathCompletionSite),
    Unqualified(UnqualifiedCompletionSite),
    RecordField(RecordFieldCompletionSite),
}

/// Cheap syntax facts that let completion avoid impossible scanner families.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompletionSiteSyntax {
    inside_use_item: bool,
    after_dot: bool,
    after_colon_colon: bool,
}

impl CompletionSiteSyntax {
    pub(crate) fn new(inside_use_item: bool, after_dot: bool, after_colon_colon: bool) -> Self {
        Self {
            inside_use_item,
            after_dot,
            after_colon_colon,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DotCompletionSite {
    source: IndexedMemberAccessSite,
}

impl DotCompletionSite {
    fn new(source: IndexedMemberAccessSite) -> Self {
        Self { source }
    }

    pub(crate) fn replace_span(&self) -> Span {
        self.source.member_prefix_span()
    }

    pub(crate) fn source(&self) -> IndexedMemberAccessSite {
        self.source
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathCompletionSite {
    source: IndexedQualifiedPathSite,
}

impl PathCompletionSite {
    fn new(source: IndexedQualifiedPathSite) -> Self {
        Self { source }
    }

    pub(crate) fn replace_span(&self) -> Span {
        self.source.member_prefix_span()
    }

    pub(crate) fn context(&self) -> PathCompletionContext {
        match self.source.scope() {
            IndexedQualifiedPathScope::Body { namespace, .. } => match namespace {
                IndexedNameNamespace::Types => PathCompletionContext::Type,
                IndexedNameNamespace::Values => PathCompletionContext::Value,
            },
            IndexedQualifiedPathScope::Import { .. } => PathCompletionContext::Import,
        }
    }

    pub(crate) fn source(&self) -> &IndexedQualifiedPathSite {
        &self.source
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathCompletionContext {
    Type,
    Value,
    Import,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnqualifiedCompletionSite {
    source: IndexedUnqualifiedNameSite,
}

impl UnqualifiedCompletionSite {
    fn new(source: IndexedUnqualifiedNameSite) -> Self {
        Self { source }
    }

    pub(crate) fn replace_span(&self) -> Span {
        self.source.member_prefix_span()
    }

    pub(crate) fn context(&self) -> UnqualifiedCompletionContext {
        match self.source.scope() {
            IndexedUnqualifiedNameScope::Body { namespace, .. } => match namespace {
                IndexedNameNamespace::Types => UnqualifiedCompletionContext::Type,
                IndexedNameNamespace::Values => UnqualifiedCompletionContext::Value,
            },
            IndexedUnqualifiedNameScope::Import { .. } => UnqualifiedCompletionContext::Import,
        }
    }

    pub(crate) fn includes_keyword_overlay(&self) -> bool {
        matches!(self.context(), UnqualifiedCompletionContext::Value)
    }

    pub(crate) fn source(&self) -> &IndexedUnqualifiedNameSite {
        &self.source
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnqualifiedCompletionContext {
    Type,
    Value,
    Import,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordFieldCompletionSite {
    source: IndexedRecordFieldListSite,
}

impl RecordFieldCompletionSite {
    fn new(source: IndexedRecordFieldListSite) -> Self {
        Self { source }
    }

    pub(crate) fn replace_span(&self) -> Span {
        self.source.member_prefix_span()
    }

    pub(crate) fn source(&self) -> &IndexedRecordFieldListSite {
        &self.source
    }
}

pub(crate) struct CompletionSiteDetector<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> CompletionSiteDetector<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    /// Classifies the cursor offset by asking the scanner that owns each syntax shape.
    pub(crate) fn site_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
        syntax: Option<CompletionSiteSyntax>,
    ) -> anyhow::Result<Option<CompletionSite>> {
        let source = SourceFactsView::new(self.db);
        if let Some(syntax) = syntax {
            if syntax.inside_use_item {
                if let Some(site) = source.import_qualified_path_site_at(target, file_id, offset)? {
                    return Ok(Some(CompletionSite::Path(PathCompletionSite::new(site))));
                }

                return Ok(source
                    .import_unqualified_name_site_at(target, file_id, offset)?
                    .map(UnqualifiedCompletionSite::new)
                    .map(CompletionSite::Unqualified));
            }
            if syntax.after_dot {
                return Ok(source
                    .member_access_site_at(target, file_id, offset)?
                    .map(DotCompletionSite::new)
                    .map(CompletionSite::Dot));
            }
            if syntax.after_colon_colon {
                return Ok(source
                    .body_qualified_path_site_at(target, file_id, offset)?
                    .map(PathCompletionSite::new)
                    .map(CompletionSite::Path));
            }
        }

        // Without a decisive syntax hint, ask scanners in the order that preserves the most
        // specific source interpretation: member access, qualified path, record field, lexical
        // body name, then import path fallback.
        if let Some(site) = source.member_access_site_at(target, file_id, offset)? {
            return Ok(Some(CompletionSite::Dot(DotCompletionSite::new(site))));
        }

        if let Some(site) = source.body_qualified_path_site_at(target, file_id, offset)? {
            return Ok(Some(CompletionSite::Path(PathCompletionSite::new(site))));
        }

        if let Some(site) = source.record_field_list_site_at(target, file_id, offset)? {
            return Ok(Some(CompletionSite::RecordField(
                RecordFieldCompletionSite::new(site),
            )));
        }

        if let Some(site) = source.body_unqualified_name_site_at(target, file_id, offset)? {
            return Ok(Some(CompletionSite::Unqualified(
                UnqualifiedCompletionSite::new(site),
            )));
        }

        if let Some(site) = source.import_qualified_path_site_at(target, file_id, offset)? {
            return Ok(Some(CompletionSite::Path(PathCompletionSite::new(site))));
        }

        Ok(source
            .import_unqualified_name_site_at(target, file_id, offset)?
            .map(UnqualifiedCompletionSite::new)
            .map(CompletionSite::Unqualified))
    }
}
