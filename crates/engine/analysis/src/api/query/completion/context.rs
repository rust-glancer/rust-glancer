//! Cursor-site classification for completion requests.

use rg_body_ir::{
    DotCompletionSite, PathCompletionSite, RecordFieldCompletionSite, UnqualifiedCompletionSite,
};
use rg_def_map::{DefMapPathCompletionSite, DefMapUnqualifiedCompletionSite, TargetRef};
use rg_parse::FileId;

use crate::Analysis;

use super::syntax::CompletionSyntaxContext;

/// Cursor shape recognized before semantic completion rendering.
pub(super) enum CompletionContext {
    /// Member access, such as `user.na$0`.
    DotCompletionSite(DotCompletionSite),
    /// Body path position, such as `let value = crate::$0`.
    BodyPathCompletionSite(PathCompletionSite),
    /// Body lexical position, such as `let value = inp$0`.
    BodyUnqualifiedCompletionSite(UnqualifiedCompletionSite),
    /// Record field position, such as `User { na$0 }`.
    RecordFieldCompletionSite(RecordFieldCompletionSite),
    /// Import path position, such as `use crate::api::$0`.
    UsePathCompletionSite(DefMapPathCompletionSite),
    /// Import root position, such as `use st$0`.
    UseUnqualifiedCompletionSite(DefMapUnqualifiedCompletionSite),
}

impl CompletionContext {
    /// Classifies the cursor offset by asking the scanner that owns each syntax
    /// shape: Body IR for expressions and type annotations, DefMap for imports.
    pub(super) fn at(
        analysis: &Analysis<'_>,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
        syntax: Option<&CompletionSyntaxContext<'_>>,
    ) -> anyhow::Result<Option<Self>> {
        if let Some(syntax) = syntax {
            // Syntax context can cheaply rule out whole scanner families. The
            // scanners still own semantic site construction and recovery.
            if syntax.inside_use_item() {
                return Self::use_context_at(analysis, target, file_id, offset);
            }
            if syntax.after_dot() {
                return Self::dot_context_at(analysis, target, file_id, offset);
            }
            if syntax.after_colon_colon() {
                return Self::body_path_context_at(analysis, target, file_id, offset);
            }
        }

        Self::general_context_at(analysis, target, file_id, offset)
    }

    fn general_context_at(
        analysis: &Analysis<'_>,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<Self>> {
        if let Some(site) = analysis
            .body_ir
            .dot_completion_site(target, file_id, offset)?
        {
            return Ok(Some(Self::DotCompletionSite(site)));
        }

        if let Some(site) = analysis
            .body_ir
            .path_completion_site(target, file_id, offset)?
        {
            return Ok(Some(Self::BodyPathCompletionSite(site)));
        }

        if let Some(site) = analysis
            .body_ir
            .record_field_completion_site(target, file_id, offset)?
        {
            return Ok(Some(Self::RecordFieldCompletionSite(site)));
        }

        if let Some(site) = analysis
            .body_ir
            .unqualified_completion_site(target, file_id, offset)?
        {
            return Ok(Some(Self::BodyUnqualifiedCompletionSite(site)));
        }

        if let Some(site) = analysis
            .def_map
            .path_completion_site(target, file_id, offset)?
        {
            return Ok(Some(Self::UsePathCompletionSite(site)));
        }

        Ok(analysis
            .def_map
            .unqualified_completion_site(target, file_id, offset)?
            .map(Self::UseUnqualifiedCompletionSite))
    }

    fn dot_context_at(
        analysis: &Analysis<'_>,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<Self>> {
        Ok(analysis
            .body_ir
            .dot_completion_site(target, file_id, offset)?
            .map(Self::DotCompletionSite))
    }

    fn body_path_context_at(
        analysis: &Analysis<'_>,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<Self>> {
        Ok(analysis
            .body_ir
            .path_completion_site(target, file_id, offset)?
            .map(Self::BodyPathCompletionSite))
    }

    fn use_context_at(
        analysis: &Analysis<'_>,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<Self>> {
        if let Some(site) = analysis
            .def_map
            .path_completion_site(target, file_id, offset)?
        {
            return Ok(Some(Self::UsePathCompletionSite(site)));
        }

        Ok(analysis
            .def_map
            .unqualified_completion_site(target, file_id, offset)?
            .map(Self::UseUnqualifiedCompletionSite))
    }
}
