//! Reference planning, document highlights, and rename queries.
//!
//! Cross-file reference work has a two-step shape. First, resolve the cursor far enough to find
//! candidate crates and source criteria. Use those criteria to prefilter files before running
//! the semantic query over the selected sources.
//!
//! Rename uses the same search plan, but has a stricter publication boundary: every target must be
//! workspace-owned and every source span must still contain the text analysis expected to replace.
//! A mismatch rejects the whole edit instead of publishing a partial rename.

use std::{path::Path, time::Instant};

use anyhow::Context as _;
use gen_lsp_types::{DocumentHighlight, Location, Position, PrepareRenameResult, WorkspaceEdit};
use rg_analysis::{
    ReferenceQuery, ReferenceSearchFile, ReferenceSearchKind, RenameEdit, RenameTarget,
};
use rg_ir_model::CrateRef;
use rg_lsp_proto::DocumentPositionSnapshot;
use rg_project::{FileContext, ProjectSnapshot};
use rg_std::UniqueVec;

use super::{DocumentSelection, QueryCancellation, QueryRunner};
use crate::proto::{
    navigation::SourceLocations, references as references_proto, rename as rename_proto,
};

/// Source coverage selected before a reference-like query can run.
///
/// `targets` is the conservative crate scope. `files = Some(...)` narrows the scan to concrete
/// crate/file pairs. `None` leaves the full target scope available to the query.
///
/// For `struct User`, Find References also searches files containing `Self`. Rename only needs
/// candidate spellings of `User`. Analysis resolves the occurrences in the selected files.
#[derive(Debug, Clone)]
struct ReferenceSearchPlan {
    targets: Vec<CrateRef>,
    files: Option<Vec<ReferenceSearchFile>>,
}

impl ReferenceSearchPlan {
    /// Turn the selected files or targets into the matching `rg_analysis` search scope.
    fn query(&self, include_declaration: bool) -> ReferenceQuery<'_> {
        match self.files.as_deref() {
            Some(files) => ReferenceQuery::find_references_in_files(files, include_declaration),
            None => ReferenceQuery::find_references(&self.targets, include_declaration),
        }
    }
}

impl QueryRunner<'_> {
    fn reference_search_plan_for_position(
        snapshot: ProjectSnapshot<'_>,
        path: &Path,
        position: Position,
        kind: ReferenceSearchKind,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Option<(FileContext, u32, ReferenceSearchPlan)>> {
        let Some((context, offset)) = Self::file_offset(snapshot, path, position)? else {
            return Ok(None);
        };
        let analysis = snapshot.analysis();
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
        let selection = analysis
            .reference_source_selection(context.crate_ref, context.file, offset, kind)
            .context("collect reference source criteria")?;
        let files = snapshot
            .reference_search_files(&targets, &selection, &cancellation.token())
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
        input: DocumentPositionSnapshot,
        include_declaration: bool,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Vec<Location>> {
        self.with_editor_snapshot(input.snapshot().editor(), cancellation, |snapshot| {
            let document = input.document();
            let path = document.source_path().to_path_buf();
            let position = input.position();
            let started = Instant::now();
            let Some((context, offset, search_plan)) = Self::reference_search_plan_for_position(
                snapshot,
                &path,
                position,
                ReferenceSearchKind::FindReferences,
                cancellation,
            )
            .context("plan references query")?
            else {
                return Ok(Vec::new());
            };
            let analysis = snapshot.analysis();
            let mut locations = UniqueVec::new();
            let sources = SourceLocations::new(snapshot, input.snapshot().editor());

            // The search still visits several crates, which can share physical source locations.
            let reference_query = search_plan.query(include_declaration);
            for reference in analysis
                .references(context.crate_ref, context.file, offset, reference_query)
                .context("find references")?
            {
                rg_std::check_cancel!(cancellation, "reference locations");
                let Some(location) = sources
                    .for_source(reference.file_id, Some(reference.span))
                    .context("convert reference location")?
                else {
                    continue;
                };
                locations.push(location);
            }

            let mut locations = locations.into_vec();
            locations.sort_by(|left, right| {
                left.uri
                    .as_str()
                    .cmp(right.uri.as_str())
                    .then_with(|| left.range.start.cmp(&right.range.start))
                    .then_with(|| left.range.end.cmp(&right.range.end))
            });

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
        })
    }

    /// Return a rename range only for a workspace-owned target whose source still matches.
    pub(crate) fn prepare_rename(
        &mut self,
        input: DocumentPositionSnapshot,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Option<PrepareRenameResult>> {
        self.with_editor_snapshot(input.snapshot().editor(), cancellation, |snapshot| {
            let document = input.document();
            let path = document.source_path().to_path_buf();
            let position = input.position();
            let started = Instant::now();
            let Some((context, offset)) = Self::file_offset(snapshot, &path, position)? else {
                return Ok(None);
            };
            if !snapshot.package_is_workspace_member(context.crate_ref.package) {
                return Ok(None);
            }
            let analysis = snapshot.analysis();
            let Some(rename_target) = analysis
                .prepare_rename(context.crate_ref, context.file, offset)
                .context("resolve rename target")?
            else {
                return Ok(None);
            };
            if !Self::rename_target_matches_source(snapshot, &rename_target)? {
                return Ok(None);
            }
            let response = Some(rename_proto::prepare_rename(snapshot, rename_target)?);

            tracing::trace!(
                path = %path.display(),
                line = position.line,
                character = position.character,
                has_result = response.is_some(),
                elapsed_ms = started.elapsed().as_millis(),
                "prepare rename query finished"
            );

            Ok(response)
        })
    }

    /// Build one all-or-nothing workspace edit for the selected symbol.
    ///
    /// The scan includes declarations, deduplicates identical edits reached through shared source,
    /// and verifies all old text after analysis before converting anything to protocol edits.
    pub(crate) fn rename(
        &mut self,
        input: DocumentPositionSnapshot,
        new_name: String,
        versioned_document_edits: bool,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Option<WorkspaceEdit>> {
        // Without versioned document changes a client cannot guard edits to dirty buffers.
        if !versioned_document_edits {
            self.project
                .saved_project()?
                .with_snapshot(cancellation.token(), |snapshot| {
                    for document in input.snapshot().editor().documents() {
                        let unchanged = if let Some(context) =
                            Self::file_context(snapshot, document.source_path())?
                        {
                            snapshot.sources().text(context.file)?.as_ref() == document.text()
                        } else {
                            false
                        };
                        if !unchanged {
                            return Err(super::lifecycle::SaveRequired(
                                document.path().to_path_buf(),
                            )
                            .into());
                        }
                    }
                    Ok(())
                })?;
        }
        self.with_editor_snapshot(input.snapshot().editor(), cancellation, |snapshot| {
            let document = input.document();
            let path = document.source_path().to_path_buf();
            let position = input.position();
            let started = Instant::now();
            let Some((context, offset, search_plan)) = Self::reference_search_plan_for_position(
                snapshot,
                &path,
                position,
                ReferenceSearchKind::Rename,
                cancellation,
            )
            .context("plan rename references")?
            else {
                return Ok(None);
            };
            if !snapshot.package_is_workspace_member(context.crate_ref.package) {
                return Ok(None);
            }
            let analysis = snapshot.analysis();
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
            if !Self::rename_target_matches_source(snapshot, &rename_result.target)? {
                return Ok(None);
            }
            let edits = rename_result.edits;

            let edit = match Self::verified_rename_edits(snapshot, edits, cancellation)
                .context("verify rename edit sources")?
            {
                Some(edits) if !edits.is_empty() => rename_proto::workspace_edit(
                    snapshot,
                    input.snapshot().editor(),
                    edits,
                    versioned_document_edits,
                    &cancellation.token(),
                )
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

            Ok(edit)
        })
    }

    /// Reuse file-scoped reference search to highlight occurrences in the requested document.
    pub(crate) fn document_highlight(
        &mut self,
        input: DocumentPositionSnapshot,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Vec<DocumentHighlight>> {
        let (document, position) = input.into_parts();
        let path = document.document().source_path().to_path_buf();
        let started = Instant::now();
        self.with_document_analysis(
            "document_highlight",
            &document,
            DocumentSelection::Position(position),
            cancellation,
            |current| {
                let Some(current) = current else {
                    return Ok(Vec::new());
                };
                let mut highlights = Vec::new();
                let offset = current.selection.start;

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
                    highlights.push(references_proto::document_highlight(
                        current.line_index,
                        reference.span,
                    ));
                }

                tracing::trace!(
                    path = %path.display(),
                    line = position.line,
                    character = position.character,
                    result_count = highlights.len(),
                    elapsed_ms = started.elapsed().as_millis(),
                    "document highlight query finished"
                );

                Ok(highlights)
            },
        )
        .context("prepare document highlights")
    }

    /// Check that the selected declaration still contains the placeholder seen by analysis.
    fn rename_target_matches_source(
        snapshot: ProjectSnapshot<'_>,
        target: &RenameTarget,
    ) -> anyhow::Result<bool> {
        Ok(snapshot
            .sources()
            .text_for_span(target.file_id, target.span)
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
                .sources()
                .text_for_span(edit.file_id, edit.span)
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
