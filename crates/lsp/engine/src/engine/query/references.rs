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
use rg_analysis::{
    Analysis as QueryAnalysis, ReferenceQuery, ReferenceSearchFile, RenameEdit, RenameTarget,
    SavedSourceRelationship,
};
use rg_ir_model::CrateRef;
use rg_lsp_proto::{
    DocumentPositionSnapshot, DocumentQueryCoverage, DocumentQueryResult, GlobalOperationResult,
    GlobalPositionSnapshot,
};
use rg_project::{AnalysisSurface, FileContext, ProjectSnapshot};
use rg_std::UniqueVec;

use super::{QueryCancellation, QueryRunner};
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
    /// Materialize the union of exact-file and whole-crate coverage selected by all contexts.
    fn ensure_reference_plans(
        &mut self,
        query: &'static str,
        plans: &[ReferenceSearchPlan],
    ) -> anyhow::Result<()> {
        let started = Instant::now();
        let mut files = UniqueVec::<(rg_def_map::PackageSlot, rg_parse::FileId)>::new();
        let mut targets = UniqueVec::<CrateRef>::new();

        // `None` means the scan needs whole crates. A present file list means text prefiltering has
        // already narrowed that portion of the scan to exact files.
        for plan in plans {
            match &plan.files {
                Some(plan_files) => {
                    files.extend(
                        plan_files
                            .iter()
                            .map(|file| (file.crate_ref.package, file.file_id)),
                    );
                }
                None => targets.extend(plan.targets.iter().copied()),
            }
        }

        let files = files.into_vec();
        let targets = targets.into_vec();
        self.project
            .materialize_saved_project(AnalysisSurface::FilesAndCrates {
                files: &files,
                crates: &targets,
            })
            .with_context(|| format!("prepare {query} reference scan"))?;
        tracing::trace!(
            query,
            file_count = files.len(),
            target_count = targets.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "analysis surface prepared for reference scan"
        );
        Ok(())
    }

    /// Decide what a reference query must scan before loading its deferred package data.
    ///
    /// Loading can replace package payloads, so references borrowed from the first snapshot cannot
    /// survive that step. Keep only the owned scan plan, take a new snapshot after loading, and
    /// resolve the cursor again there.
    fn reference_search_plans_for_position(
        &mut self,
        path: &Path,
        position: ls_types::Position,
    ) -> anyhow::Result<Vec<ReferenceSearchPlan>> {
        let project = self
            .project
            .saved_snapshot()
            .context("borrow saved project for reference planning")?;
        let crate_offsets = Self::crate_offsets(project, path, position)
            .context("resolve reference-search position")?;
        let analysis = project
            .full_analysis()
            .context("load reference-search analysis")?;
        let mut plans = Vec::new();

        for (context, crate_ref, offset) in crate_offsets {
            plans.push(
                Self::reference_search_plan(project, &analysis, &context, crate_ref, offset)
                    .context("build reference-search plan")?,
            );
        }

        Ok(plans)
    }

    /// Find references across all crate contexts selected for this cursor.
    pub(crate) fn references(
        &mut self,
        input: GlobalPositionSnapshot,
        include_declaration: bool,
    ) -> anyhow::Result<GlobalOperationResult<Vec<ls_types::Location>>> {
        if let Some(path) = self.save_required_for_global_operation(&input)? {
            return Ok(GlobalOperationResult::save_required(path));
        }
        let document = Self::global_operation_target(&input)?;
        let path = document.source_path().to_path_buf();
        let position = input.position();
        let started = Instant::now();
        self.ensure_path("references", &path)
            .context("prepare references path")?;
        let search_plans = self
            .reference_search_plans_for_position(&path, position)
            .context("plan references query")?;
        self.ensure_reference_plans("references", &search_plans)
            .context("materialize references search")?;
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for references")?;
        let crate_offsets = Self::crate_offsets(snapshot, &path, position)
            .context("resolve references position")?;
        let analysis = snapshot
            .full_analysis()
            .context("load references analysis")?;
        let mut locations = UniqueVec::new();

        for (context, crate_ref, offset) in crate_offsets {
            let search_plan =
                Self::reference_search_plan(snapshot, &analysis, &context, crate_ref, offset)
                    .context("build references search plan")?;
            let reference_query = search_plan.query(include_declaration);

            for reference in analysis
                .references(crate_ref, context.file, offset, reference_query)
                .context("find references")?
            {
                let Some(location) = references_proto::location_for_reference(snapshot, &reference)
                    .context("convert reference location")?
                else {
                    continue;
                };
                locations.push(location);
            }
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

        Ok(GlobalOperationResult::ready(locations))
    }

    /// Return a rename range only for a workspace-owned target whose source still matches.
    pub(crate) fn prepare_rename(
        &mut self,
        input: GlobalPositionSnapshot,
    ) -> anyhow::Result<GlobalOperationResult<Option<ls_types::PrepareRenameResponse>>> {
        if let Some(path) = self.save_required_for_global_operation(&input)? {
            return Ok(GlobalOperationResult::save_required(path));
        }
        let document = Self::global_operation_target(&input)?;
        let path = document.source_path().to_path_buf();
        let position = input.position();
        let started = Instant::now();
        self.ensure_path("prepare_rename", &path)
            .context("prepare rename path")?;
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for prepare rename")?;
        let crate_offsets = Self::crate_offsets(snapshot, &path, position)
            .context("resolve prepare-rename position")?;
        let analysis_crates = crate_offsets
            .iter()
            .map(|(_, crate_ref, _)| *crate_ref)
            .collect::<Vec<_>>();
        let analysis = snapshot
            .analysis_for_crates(&analysis_crates)
            .context("load prepare-rename analysis")?;
        let mut response = None;

        for (context, crate_ref, offset) in crate_offsets {
            if !snapshot.package_is_workspace_member(context.package) {
                continue;
            }
            let Some(rename_target) = analysis
                .prepare_rename(crate_ref, context.file, offset)
                .context("resolve rename target")?
            else {
                continue;
            };
            if !Self::rename_target_matches_source(snapshot, context.package, &rename_target)
                .context("verify rename target source")?
            {
                continue;
            }

            response = rename_proto::prepare_rename(snapshot, context.package, rename_target)
                .map(Some)
                .context("convert prepare-rename response")?;
            break;
        }

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            has_result = response.is_some(),
            elapsed_ms = started.elapsed().as_millis(),
            "prepare rename query finished"
        );

        Ok(GlobalOperationResult::ready(response))
    }

    /// Build one all-or-nothing workspace edit from every matching crate context.
    ///
    /// The scan includes declarations, deduplicates identical edits reached through shared source,
    /// and verifies all old text after analysis before converting anything to protocol edits.
    pub(crate) fn rename(
        &mut self,
        input: GlobalPositionSnapshot,
        new_name: String,
    ) -> anyhow::Result<GlobalOperationResult<Option<ls_types::WorkspaceEdit>>> {
        if let Some(path) = self.save_required_for_global_operation(&input)? {
            return Ok(GlobalOperationResult::save_required(path));
        }
        let document = Self::global_operation_target(&input)?;
        let path = document.source_path().to_path_buf();
        let position = input.position();
        let started = Instant::now();
        self.ensure_path("rename", &path)
            .context("prepare rename path")?;
        let search_plans = self
            .reference_search_plans_for_position(&path, position)
            .context("plan rename references")?;
        self.ensure_reference_plans("rename", &search_plans)
            .context("materialize rename search")?;
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for rename")?;
        let crate_offsets =
            Self::crate_offsets(snapshot, &path, position).context("resolve rename position")?;
        let analysis = snapshot.full_analysis().context("load rename analysis")?;
        let mut edits = UniqueVec::new();

        for (context, crate_ref, offset) in crate_offsets {
            if !snapshot.package_is_workspace_member(context.package) {
                continue;
            }
            let search_plan =
                Self::reference_search_plan(snapshot, &analysis, &context, crate_ref, offset)
                    .context("build rename search plan")?;
            let reference_query = search_plan.query(true);
            let Some(rename_result) = analysis
                .rename(crate_ref, context.file, offset, &new_name, reference_query)
                .context("compute rename edits")?
            else {
                continue;
            };

            if !Self::rename_target_matches_source(snapshot, context.package, &rename_result.target)
                .context("verify rename target source")?
            {
                continue;
            }
            edits.extend(rename_result.edits);
        }

        let edit = match Self::verified_rename_edits(snapshot, edits)
            .context("verify rename edit sources")?
        {
            Some(edits) if !edits.is_empty() => rename_proto::workspace_edit(snapshot, edits)
                .map(Some)
                .context("build rename workspace edit")?,
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

        Ok(GlobalOperationResult::ready(edit))
    }

    /// Reuse file-scoped reference search to highlight occurrences in the requested document.
    pub(crate) fn document_highlight(
        &mut self,
        input: DocumentPositionSnapshot,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<DocumentQueryResult<Vec<ls_types::DocumentHighlight>>> {
        let (document, position) = input.into_parts();
        let path = document.source_path().to_path_buf();
        let started = Instant::now();
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for document highlights")?;
        let Some(current) =
            Self::current_position_analysis(snapshot, &document, position, cancellation)
                .context("prepare current document highlights")?
        else {
            return Ok(DocumentQueryResult::new(
                Vec::new(),
                DocumentQueryCoverage::Partial,
            ));
        };
        let mut highlights = UniqueVec::new();
        let mut every_target_has_safe_coordinates = true;

        for target in &current.targets {
            let has_exact_body = current.target_has_exact_body(target);
            // Rebuilt body spans already use editor coordinates. Saved item spans are safe too
            // when the editor text is byte-for-byte equal to the source that produced them.
            let matches_saved = !has_exact_body
                && current
                    .analysis
                    .current_source_relationship(target.context.package, target.context.file)
                    == Some(SavedSourceRelationship::Exact);
            let has_safe_coordinates = has_exact_body || matches_saved;
            every_target_has_safe_coordinates &= has_safe_coordinates;
            if !has_safe_coordinates {
                continue;
            }
            for reference in current
                .analysis
                .references(
                    target.crate_ref,
                    target.context.file,
                    current.offset,
                    ReferenceQuery::file_scoped(target.crate_ref, target.context.file),
                )
                .context("find current document references")?
            {
                if reference.crate_ref.package != target.context.package
                    || reference.file_id != target.context.file
                {
                    continue;
                }
                highlights.push(references_proto::document_highlight_for_current_document(
                    current.source.line_index(),
                    reference.span,
                ));
            }
        }
        let highlights = highlights.into_vec();

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            result_count = highlights.len(),
            exact_body = current.coverage.is_exact(),
            elapsed_ms = started.elapsed().as_millis(),
            "document highlight query finished"
        );

        let coverage = if current.coverage.is_exact() || every_target_has_safe_coordinates {
            DocumentQueryCoverage::Exact
        } else {
            DocumentQueryCoverage::Partial
        };
        Ok(DocumentQueryResult::new(highlights, coverage))
    }

    /// Resolve the declaration scope and choose a safe text-prefiltered file set when possible.
    fn reference_search_plan(
        snapshot: ProjectSnapshot<'_>,
        analysis: &QueryAnalysis<'_>,
        context: &FileContext,
        crate_ref: CrateRef,
        offset: u32,
    ) -> anyhow::Result<ReferenceSearchPlan> {
        let declaration_targets = analysis
            .goto_definition(crate_ref, context.file, offset)
            .context("resolve reference declaration")?
            .into_iter()
            .map(|target| target.crate_ref)
            .collect::<Vec<_>>();
        let targets = snapshot.reference_search_crates(context.package, &declaration_targets);
        let labels = analysis
            .reference_search_labels(crate_ref, context.file, offset)
            .context("collect reference-search labels")?;
        let files = snapshot
            .reference_search_files_matching_labels(&targets, &labels)
            .context("prefilter reference-search files")?;

        Ok(ReferenceSearchPlan { targets, files })
    }

    /// Check that the selected declaration still contains the placeholder seen by analysis.
    fn rename_target_matches_source(
        snapshot: ProjectSnapshot<'_>,
        package: rg_def_map::PackageSlot,
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
        edits: UniqueVec<RenameEdit>,
    ) -> anyhow::Result<Option<Vec<RenameEdit>>> {
        for edit in &edits {
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

        Ok(Some(edits.into_vec()))
    }
}
