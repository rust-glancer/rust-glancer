//! Cursor-site classification for completion requests.

use rg_body_ir::{
    DotCompletionSite, PathCompletionSite, RecordFieldCompletionSite, UnqualifiedCompletionSite,
};
use rg_def_map::{DefMapPathCompletionSite, DefMapUnqualifiedCompletionSite};

use crate::Analysis;

use super::{CompletionQuery, syntax::CompletionSyntaxContext};

/// Cursor shape recognized before semantic completion rendering.
pub(super) enum CompletionContext {
    /// Member access, such as `user.na$0`.
    Dot(DotCompletionSite),
    /// Body path position, such as `let value = crate::$0`.
    BodyPath(PathCompletionSite),
    /// Body lexical position, such as `let value = inp$0`.
    BodyUnqualified(UnqualifiedCompletionSite),
    /// Record field position, such as `User { na$0 }`.
    RecordField(RecordFieldCompletionSite),
    /// Import path position, such as `use crate::api::$0`.
    UsePath(DefMapPathCompletionSite),
    /// Import root position, such as `use st$0`.
    UseUnqualified(DefMapUnqualifiedCompletionSite),
}

impl CompletionContext {
    /// Classifies the cursor offset by asking the scanner that owns each syntax
    /// shape: Body IR for expressions and type annotations, DefMap for imports.
    pub(super) fn at(
        analysis: &Analysis<'_>,
        query: CompletionQuery<'_>,
        syntax: Option<&CompletionSyntaxContext<'_>>,
    ) -> anyhow::Result<Option<Self>> {
        if let Some(syntax) = syntax {
            // Syntax context can cheaply rule out whole scanner families. The
            // scanners still own semantic site construction and recovery.
            if syntax.inside_use_item() {
                return Self::use_context_at(analysis, query);
            }
            if syntax.after_dot() {
                return Self::dot_context_at(analysis, query);
            }
            if syntax.after_colon_colon() {
                return Self::body_path_context_at(analysis, query);
            }
        }

        Self::general_context_at(analysis, query)
    }

    fn general_context_at(
        analysis: &Analysis<'_>,
        query: CompletionQuery<'_>,
    ) -> anyhow::Result<Option<Self>> {
        if let Some(site) =
            analysis
                .body_ir
                .dot_completion_site(query.target, query.file_id, query.offset)?
        {
            return Ok(Some(Self::Dot(site)));
        }

        if let Some(site) =
            analysis
                .body_ir
                .path_completion_site(query.target, query.file_id, query.offset)?
        {
            return Ok(Some(Self::BodyPath(site)));
        }

        if let Some(site) = analysis.body_ir.record_field_completion_site(
            query.target,
            query.file_id,
            query.offset,
        )? {
            return Ok(Some(Self::RecordField(site)));
        }

        if let Some(site) = analysis.body_ir.unqualified_completion_site(
            query.target,
            query.file_id,
            query.offset,
        )? {
            return Ok(Some(Self::BodyUnqualified(site)));
        }

        if let Some(site) =
            analysis
                .def_map
                .path_completion_site(query.target, query.file_id, query.offset)?
        {
            return Ok(Some(Self::UsePath(site)));
        }

        Ok(analysis
            .def_map
            .unqualified_completion_site(query.target, query.file_id, query.offset)?
            .map(Self::UseUnqualified))
    }

    fn dot_context_at(
        analysis: &Analysis<'_>,
        query: CompletionQuery<'_>,
    ) -> anyhow::Result<Option<Self>> {
        Ok(analysis
            .body_ir
            .dot_completion_site(query.target, query.file_id, query.offset)?
            .map(Self::Dot))
    }

    fn body_path_context_at(
        analysis: &Analysis<'_>,
        query: CompletionQuery<'_>,
    ) -> anyhow::Result<Option<Self>> {
        Ok(analysis
            .body_ir
            .path_completion_site(query.target, query.file_id, query.offset)?
            .map(Self::BodyPath))
    }

    fn use_context_at(
        analysis: &Analysis<'_>,
        query: CompletionQuery<'_>,
    ) -> anyhow::Result<Option<Self>> {
        if let Some(site) =
            analysis
                .def_map
                .path_completion_site(query.target, query.file_id, query.offset)?
        {
            return Ok(Some(Self::UsePath(site)));
        }

        Ok(analysis
            .def_map
            .unqualified_completion_site(query.target, query.file_id, query.offset)?
            .map(Self::UseUnqualified))
    }
}
