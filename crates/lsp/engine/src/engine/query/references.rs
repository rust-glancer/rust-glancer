//! Reference planning, document highlights, and rename queries.
//!
//! Cross-file reference work has a two-step shape. First, resolve the cursor far enough to find
//! candidate crates and safe identifier labels. Those labels can often prefilter the scan to exact
//! source files. Then materialize the planned files or whole crates and run the real semantic
//! query against a fresh snapshot.
//!
//! Rename uses the same search plan, but has a stricter publication boundary: every target must be
//! workspace-owned and every source span must still contain the text analysis expected to replace.
//! A mismatch rejects the whole edit instead of publishing a partial rename.

use std::{path::Path, time::Instant};

use anyhow::Context as _;
use rg_analysis::{ReferenceQuery, ReferenceSearchFile, RenameEdit, RenameTarget};
use rg_ir_model::{CrateRef, PackageSlot};
use rg_lsp_proto::{DocumentPositionSnapshot, GlobalPositionSnapshot};
use rg_project::{AnalysisSurface, FileContext, ProjectSnapshot};
use rg_std::UniqueVec;

use super::{DocumentSelection, QueryCancellation, QueryRunError, QueryRunner};
use crate::proto::{references as references_proto, rename as rename_proto};

/// Source coverage selected before a reference-like query can run.
///
/// `targets` is the conservative crate scope. `files = Some(...)` means identifier prefiltering
/// safely narrowed that scope to concrete crate/file pairs. `None` means no safe label was
/// available, so the query must materialize and scan every target crate.
///
/// For `struct User` and a cursor on `User`, a plan can keep only files containing that identifier
/// token. Semantic reference matching still proves which of those textual occurrences mean this
/// particular `User`.
#[derive(Debug, Clone)]
struct ReferenceSearchPlan {
    targets: Vec<CrateRef>,
    files: Option<Vec<ReferenceSearchFile>>,
}

impl ReferenceSearchPlan {
    /// Turn materialized coverage into the matching `rg_analysis` search scope.
    fn query(&self, include_declaration: bool) -> ReferenceQuery<'_> {
        match self.files.as_deref() {
            Some(files) => ReferenceQuery::find_references_in_files(files, include_declaration),
            None => ReferenceQuery::find_references(&self.targets, include_declaration),
        }
    }
}

impl QueryRunner<'_> {
    /// Materialize the one origin plan, retaining its cross-crate search scope.
    fn ensure_reference_plan(
        &mut self,
        query: &'static str,
        plan: &ReferenceSearchPlan,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<()> {
        match &plan.files {
            Some(files) => {
                let files = files
                    .iter()
                    .map(|file| (file.crate_ref, file.file_id))
                    .collect::<Vec<_>>();
                self.project.materialize_saved_project(
                    AnalysisSurface::Files(&files),
                    &cancellation.token(),
                )
            }
            None => self.project.materialize_saved_project(
                AnalysisSurface::Crates(&plan.targets),
                &cancellation.token(),
            ),
        }
        .with_context(|| format!("prepare {query} reference scan"))?;
        Ok(())
    }

    /// Keep owned identities and scan coverage across materialization, which replaces saved payloads.
    fn reference_search_plan_for_position(
        &mut self,
        path: &Path,
        position: gen_lsp_types::Position,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Option<(FileContext, u32, ReferenceSearchPlan)>> {
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for reference planning")?;
        let Some((context, offset)) = Self::file_offset(snapshot, path, position)? else {
            return Ok(None);
        };
        let analysis = snapshot
            .analysis_for_crates(&[context.crate_ref], cancellation.token())
            .context("load reference-search analysis")?;
        let declaration_targets = analysis
            .goto_definition(context.crate_ref, context.file, offset)
            .context("resolve reference declaration")?
            .into_iter()
            .map(|target| target.crate_ref)
            .collect::<Vec<_>>();
        let targets = snapshot
            .reference_search_crates(
                context.crate_ref.package,
                &declaration_targets,
                &cancellation.token(),
            )
            .context("plan reference search crates")?;
        let labels = analysis
            .reference_search_labels(context.crate_ref, context.file, offset)
            .context("collect reference-search labels")?;
        let files = snapshot
            .reference_search_files_matching_labels(&targets, &labels, &cancellation.token())
            .context("prefilter reference-search files")?;
        Ok(Some((
            context,
            offset,
            ReferenceSearchPlan { targets, files },
        )))
    }

    /// Find references to the symbol resolved in the selected cursor context.
    pub(crate) fn references(
        &mut self,
        input: GlobalPositionSnapshot,
        include_declaration: bool,
        cancellation: &QueryCancellation<'_>,
    ) -> Result<Vec<gen_lsp_types::Location>, QueryRunError> {
        if let Some(path) = self
            .save_required_for_global_operation(&input, cancellation)
            .context("check references source safety")?
        {
            return Err(QueryRunError::SaveRequired(path));
        }
        let document = Self::global_operation_target(&input)?;
        let path = document.source_path().to_path_buf();
        let position = input.position();
        let started = Instant::now();
        self.ensure_path("references", &path, cancellation)
            .context("prepare references path")?;
        let Some((context, offset, search_plan)) = self
            .reference_search_plan_for_position(&path, position, cancellation)
            .context("plan references query")?
        else {
            return Ok(Vec::new());
        };
        self.ensure_reference_plan("references", &search_plan, cancellation)?;
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for references")?;
        let analysis = snapshot
            .full_analysis(cancellation.token())
            .context("load references analysis")?;
        let mut locations = UniqueVec::new();

        // The search still visits several crates, which can share physical source locations.
        let reference_query = search_plan.query(include_declaration);
        for reference in analysis
            .references(context.crate_ref, context.file, offset, reference_query)
            .context("find references")?
        {
            rg_std::check_cancel!(cancellation, "reference locations");
            let Some(location) = references_proto::location_for_reference(snapshot, &reference)
                .context("convert reference location")?
            else {
                continue;
            };
            locations.push(location);
        }

        let locations = locations.into_vec();

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            include_declaration,
            result_count = locations.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "references query finished"
        );

        Ok(locations)
    }

    /// Return a rename range only for a workspace-owned target whose source still matches.
    pub(crate) fn prepare_rename(
        &mut self,
        input: GlobalPositionSnapshot,
        cancellation: &QueryCancellation<'_>,
    ) -> Result<Option<gen_lsp_types::PrepareRenameResult>, QueryRunError> {
        if let Some(path) = self
            .save_required_for_global_operation(&input, cancellation)
            .context("check prepare-rename source safety")?
        {
            return Err(QueryRunError::SaveRequired(path));
        }
        let document = Self::global_operation_target(&input)?;
        let path = document.source_path().to_path_buf();
        let position = input.position();
        let started = Instant::now();
        self.ensure_path("prepare_rename", &path, cancellation)
            .context("prepare rename path")?;
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for prepare rename")?;
        let Some((context, offset)) = Self::file_offset(snapshot, &path, position)? else {
            return Ok(None);
        };
        if !snapshot.package_is_workspace_member(context.crate_ref.package) {
            return Ok(None);
        }
        let analysis = snapshot
            .analysis_for_crates(&[context.crate_ref], cancellation.token())
            .context("load prepare-rename analysis")?;
        let Some(rename_target) = analysis
            .prepare_rename(context.crate_ref, context.file, offset)
            .context("resolve rename target")?
        else {
            return Ok(None);
        };
        if !Self::rename_target_matches_source(snapshot, context.crate_ref.package, &rename_target)?
        {
            return Ok(None);
        }
        let response = Some(rename_proto::prepare_rename(
            snapshot,
            context.crate_ref.package,
            rename_target,
        )?);

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            has_result = response.is_some(),
            elapsed_ms = started.elapsed().as_millis(),
            "prepare rename query finished"
        );

        Ok(response)
    }

    /// Build one all-or-nothing workspace edit for the selected symbol.
    ///
    /// The scan includes declarations, deduplicates identical edits reached through shared source,
    /// and verifies all old text after analysis before converting anything to protocol edits.
    pub(crate) fn rename(
        &mut self,
        input: GlobalPositionSnapshot,
        new_name: String,
        cancellation: &QueryCancellation<'_>,
    ) -> Result<Option<gen_lsp_types::WorkspaceEdit>, QueryRunError> {
        if let Some(path) = self
            .save_required_for_global_operation(&input, cancellation)
            .context("check rename source safety")?
        {
            return Err(QueryRunError::SaveRequired(path));
        }
        let document = Self::global_operation_target(&input)?;
        let path = document.source_path().to_path_buf();
        let position = input.position();
        let started = Instant::now();
        self.ensure_path("rename", &path, cancellation)
            .context("prepare rename path")?;
        let Some((context, offset, search_plan)) = self
            .reference_search_plan_for_position(&path, position, cancellation)
            .context("plan rename references")?
        else {
            return Ok(None);
        };
        if !self
            .project
            .saved_snapshot()?
            .package_is_workspace_member(context.crate_ref.package)
        {
            return Ok(None);
        }
        self.ensure_reference_plan("rename", &search_plan, cancellation)?;
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for rename")?;
        let analysis = snapshot
            .full_analysis(cancellation.token())
            .context("load rename analysis")?;
        let Some(rename_result) = analysis
            .rename(
                context.crate_ref,
                context.file,
                offset,
                &new_name,
                search_plan.query(true),
            )
            .context("compute rename edits")?
        else {
            return Ok(None);
        };
        if !Self::rename_target_matches_source(
            snapshot,
            context.crate_ref.package,
            &rename_result.target,
        )? {
            return Ok(None);
        }
        let edits = rename_result.edits;

        let edit = match Self::verified_rename_edits(snapshot, edits, cancellation)
            .context("verify rename edit sources")?
        {
            Some(edits) if !edits.is_empty() => {
                rename_proto::workspace_edit(snapshot, edits, &cancellation.token())
                    .map(Some)
                    .context("build rename workspace edit")?
            }
            Some(_) | None => None,
        };

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            new_name = %new_name,
            has_edit = edit.is_some(),
            elapsed_ms = started.elapsed().as_millis(),
            "rename query finished"
        );

        Ok(edit)
    }

    /// Reuse file-scoped reference search to highlight occurrences in the requested document.
    pub(crate) fn document_highlight(
        &mut self,
        input: DocumentPositionSnapshot,
        cancellation: &QueryCancellation<'_>,
    ) -> Result<Vec<gen_lsp_types::DocumentHighlight>, QueryRunError> {
        let (document, position) = input.into_parts();
        let path = document.source_path().to_path_buf();
        let started = Instant::now();
        let Some(current) = self
            .document_analysis(
                "document_highlight",
                &document,
                DocumentSelection::Position(position),
                cancellation,
            )
            .context("prepare document highlights")?
        else {
            return Ok(Vec::new());
        };
        let mut highlights = Vec::new();
        let offset = current.offset();

        let target = current.target;
        rg_std::check_cancel!(cancellation, "document highlights");
        for reference in current
            .analysis
            .references(
                target.crate_ref,
                target.file,
                offset,
                ReferenceQuery::file_scoped(target.crate_ref, target.file),
            )
            .context("find current document references")?
        {
            rg_std::check_cancel!(cancellation, "highlight conversion");
            highlights.push(references_proto::document_highlight_for_current_document(
                current.source.line_index(),
                reference.span,
            ));
        }

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            result_count = highlights.len(),
            source = current.source.name(),
            elapsed_ms = started.elapsed().as_millis(),
            "document highlight query finished"
        );

        Ok(highlights)
    }

    /// Check that the selected declaration still contains the placeholder seen by analysis.
    fn rename_target_matches_source(
        snapshot: ProjectSnapshot<'_>,
        package: PackageSlot,
        target: &RenameTarget,
    ) -> anyhow::Result<bool> {
        Ok(snapshot
            .file_text_for_span(package, target.file_id, target.span)
            .context("read rename target source")?
            .is_some_and(|text| text == target.placeholder))
    }

    /// Reject the complete rename if any edit is external, missing, or stale.
    ///
    /// This final check closes the gap between semantic analysis and edit publication. Returning
    /// `None` means the caller must publish no edit at all; an empty `Some` remains a valid
    /// verified collection for the caller to classify separately.
    fn verified_rename_edits(
        snapshot: ProjectSnapshot<'_>,
        edits: Vec<RenameEdit>,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Option<Vec<RenameEdit>>> {
        for edit in &edits {
            rg_std::check_cancel!(cancellation, "rename source validation");
            // Keep this query limited to workspace-owned files. References may legitimately see
            // dependency declarations, but rename should not edit source outside this workspace.
            if !snapshot.package_is_workspace_member(edit.crate_ref.package) {
                tracing::debug!(
                    package = ?edit.crate_ref.package,
                    "rename rejected because an edit targets a non-workspace package"
                );
                return Ok(None);
            }

            let Some(text) = snapshot
                .file_text_for_span(edit.crate_ref.package, edit.file_id, edit.span)
                .context("read rename edit source")?
            else {
                tracing::debug!(
                    package = ?edit.crate_ref.package,
                    file = ?edit.file_id,
                    "rename rejected because an edit span has no source text"
                );
                return Ok(None);
            };
            if text != edit.old_text {
                tracing::debug!(
                    package = ?edit.crate_ref.package,
                    file = ?edit.file_id,
                    expected = %edit.old_text,
                    actual = %text,
                    "rename rejected because an edit span did not match the expected source text"
                );
                return Ok(None);
            }
        }

        Ok(Some(edits))
    }
}
