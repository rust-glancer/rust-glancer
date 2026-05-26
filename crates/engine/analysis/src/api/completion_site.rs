//! Completion-domain cursor sites.
//!
//! Body IR and DefMap own source scanning for their syntax shapes. Completion owns the question
//! "what kind of completion site did the cursor select?", so this module wraps storage-specific
//! sites behind a small completion vocabulary.

use rg_body_ir::{
    DotCompletionSite as BodyDotCompletionSite,
    PathCompletionNamespace as BodyPathCompletionNamespace,
    PathCompletionSite as BodyPathCompletionSite,
    RecordFieldCompletionSite as BodyRecordFieldCompletionSite,
    UnqualifiedCompletionNamespace as BodyUnqualifiedCompletionNamespace,
    UnqualifiedCompletionSite as BodyUnqualifiedCompletionSite,
};
use rg_def_map::{DefMapPathCompletionSite, DefMapUnqualifiedCompletionSite};
use rg_ir_model::TargetRef;
use rg_parse::{FileId, Span};

use crate::api::Analysis;

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
    source: BodyDotCompletionSite,
}

impl DotCompletionSite {
    fn new(source: BodyDotCompletionSite) -> Self {
        Self { source }
    }

    pub(crate) fn replace_span(&self) -> Span {
        self.source.member_prefix_span
    }

    pub(crate) fn body_site(&self) -> BodyDotCompletionSite {
        self.source
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathCompletionSite {
    source: PathCompletionSource,
}

impl PathCompletionSite {
    fn body(source: BodyPathCompletionSite) -> Self {
        Self {
            source: PathCompletionSource::Body(source),
        }
    }

    fn import(source: DefMapPathCompletionSite) -> Self {
        Self {
            source: PathCompletionSource::Import(source),
        }
    }

    pub(crate) fn replace_span(&self) -> Span {
        match &self.source {
            PathCompletionSource::Body(site) => site.member_prefix_span,
            PathCompletionSource::Import(site) => site.member_prefix_span,
        }
    }

    pub(crate) fn context(&self) -> PathCompletionContext {
        match &self.source {
            PathCompletionSource::Body(site) => match site.namespace {
                BodyPathCompletionNamespace::Types => PathCompletionContext::Type,
                BodyPathCompletionNamespace::Values => PathCompletionContext::Value,
            },
            PathCompletionSource::Import(_) => PathCompletionContext::Import,
        }
    }

    pub(crate) fn body_site(&self) -> Option<&BodyPathCompletionSite> {
        match &self.source {
            PathCompletionSource::Body(site) => Some(site),
            PathCompletionSource::Import(_) => None,
        }
    }

    pub(crate) fn import_site(&self) -> Option<&DefMapPathCompletionSite> {
        match &self.source {
            PathCompletionSource::Body(_) => None,
            PathCompletionSource::Import(site) => Some(site),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PathCompletionSource {
    Body(BodyPathCompletionSite),
    Import(DefMapPathCompletionSite),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathCompletionContext {
    Type,
    Value,
    Import,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnqualifiedCompletionSite {
    source: UnqualifiedCompletionSource,
}

impl UnqualifiedCompletionSite {
    fn body(source: BodyUnqualifiedCompletionSite) -> Self {
        Self {
            source: UnqualifiedCompletionSource::Body(source),
        }
    }

    fn import(source: DefMapUnqualifiedCompletionSite) -> Self {
        Self {
            source: UnqualifiedCompletionSource::Import(source),
        }
    }

    pub(crate) fn replace_span(&self) -> Span {
        match &self.source {
            UnqualifiedCompletionSource::Body(site) => site.member_prefix_span,
            UnqualifiedCompletionSource::Import(site) => site.member_prefix_span,
        }
    }

    pub(crate) fn context(&self) -> UnqualifiedCompletionContext {
        match &self.source {
            UnqualifiedCompletionSource::Body(site) => match site.namespace {
                BodyUnqualifiedCompletionNamespace::Types => UnqualifiedCompletionContext::Type,
                BodyUnqualifiedCompletionNamespace::Values => UnqualifiedCompletionContext::Value,
            },
            UnqualifiedCompletionSource::Import(_) => UnqualifiedCompletionContext::Import,
        }
    }

    pub(crate) fn includes_keyword_overlay(&self) -> bool {
        matches!(self.context(), UnqualifiedCompletionContext::Value)
    }

    pub(crate) fn body_site(&self) -> Option<&BodyUnqualifiedCompletionSite> {
        match &self.source {
            UnqualifiedCompletionSource::Body(site) => Some(site),
            UnqualifiedCompletionSource::Import(_) => None,
        }
    }

    pub(crate) fn import_site(&self) -> Option<&DefMapUnqualifiedCompletionSite> {
        match &self.source {
            UnqualifiedCompletionSource::Body(_) => None,
            UnqualifiedCompletionSource::Import(site) => Some(site),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UnqualifiedCompletionSource {
    Body(BodyUnqualifiedCompletionSite),
    Import(DefMapUnqualifiedCompletionSite),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnqualifiedCompletionContext {
    Type,
    Value,
    Import,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordFieldCompletionSite {
    source: BodyRecordFieldCompletionSite,
}

impl RecordFieldCompletionSite {
    fn new(source: BodyRecordFieldCompletionSite) -> Self {
        Self { source }
    }

    pub(crate) fn replace_span(&self) -> Span {
        self.source.member_prefix_span
    }

    pub(crate) fn body_site(&self) -> &BodyRecordFieldCompletionSite {
        &self.source
    }
}

pub(crate) struct CompletionSiteDetector<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> CompletionSiteDetector<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    /// Classifies the cursor offset by asking the scanner that owns each syntax shape.
    pub(crate) fn site_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
        syntax: Option<CompletionSiteSyntax>,
    ) -> anyhow::Result<Option<CompletionSite>> {
        if let Some(syntax) = syntax {
            if syntax.inside_use_item {
                if let Some(site) = self
                    .analysis
                    .def_map
                    .path_completion_site(target, file_id, offset)?
                {
                    return Ok(Some(CompletionSite::Path(PathCompletionSite::import(site))));
                }

                return Ok(self
                    .analysis
                    .def_map
                    .unqualified_completion_site(target, file_id, offset)?
                    .map(UnqualifiedCompletionSite::import)
                    .map(CompletionSite::Unqualified));
            }
            if syntax.after_dot {
                return Ok(self
                    .analysis
                    .body_ir
                    .dot_completion_site(target, file_id, offset)?
                    .map(DotCompletionSite::new)
                    .map(CompletionSite::Dot));
            }
            if syntax.after_colon_colon {
                return Ok(self
                    .analysis
                    .body_ir
                    .path_completion_site(target, file_id, offset)?
                    .map(PathCompletionSite::body)
                    .map(CompletionSite::Path));
            }
        }

        // Without a decisive syntax hint, ask scanners in the order that preserves the most
        // specific source interpretation: member access, qualified path, record field, lexical
        // body name, then import path fallback.
        if let Some(site) = self
            .analysis
            .body_ir
            .dot_completion_site(target, file_id, offset)?
        {
            return Ok(Some(CompletionSite::Dot(DotCompletionSite::new(site))));
        }

        if let Some(site) = self
            .analysis
            .body_ir
            .path_completion_site(target, file_id, offset)?
        {
            return Ok(Some(CompletionSite::Path(PathCompletionSite::body(site))));
        }

        if let Some(site) = self
            .analysis
            .body_ir
            .record_field_completion_site(target, file_id, offset)?
        {
            return Ok(Some(CompletionSite::RecordField(
                RecordFieldCompletionSite::new(site),
            )));
        }

        if let Some(site) = self
            .analysis
            .body_ir
            .unqualified_completion_site(target, file_id, offset)?
        {
            return Ok(Some(CompletionSite::Unqualified(
                UnqualifiedCompletionSite::body(site),
            )));
        }

        if let Some(site) = self
            .analysis
            .def_map
            .path_completion_site(target, file_id, offset)?
        {
            return Ok(Some(CompletionSite::Path(PathCompletionSite::import(site))));
        }

        Ok(self
            .analysis
            .def_map
            .unqualified_completion_site(target, file_id, offset)?
            .map(UnqualifiedCompletionSite::import)
            .map(CompletionSite::Unqualified))
    }
}
