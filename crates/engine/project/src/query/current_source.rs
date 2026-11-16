//! Prepares captured editor text and request-local bodies against the saved project.

use std::sync::Arc;

use anyhow::Context as _;
use rg_analysis::{Analysis, CurrentSourceView, SavedSourceRelationship};
use rg_body_ir::{CurrentSourceBuildCheckpoint, CurrentSourceBuildSummary, CurrentSourceSelection};
use rg_ir_model::{CrateRef, FileId};
use rg_parse::{CurrentSource, DeclarationAssociationIndex, LineIndex, SavedSourceReader};
use rg_source::SourceRevision;
use rg_workspace::PackageInput;

use super::ProjectSnapshot;

/// Exact saved text can reuse saved Body IR. Changed text needs request-local syntax and bodies.
#[derive(Debug)]
pub enum DocumentSourceView {
    SavedExact(LineIndex),
    Current(CurrentSourceView),
}

impl DocumentSourceView {
    pub fn line_index(&self) -> &LineIndex {
        match self {
            Self::SavedExact(line_index) => line_index,
            Self::Current(source) => source.source().line_index(),
        }
    }
}

impl<'a> ProjectSnapshot<'a> {
    /// Compare the captured bytes with the selected saved file before preparing current syntax.
    pub fn prepare_document_source(
        &self,
        target: (CrateRef, FileId),
        source: &str,
        cancellation: &rg_std::CancellationToken,
    ) -> anyhow::Result<DocumentSourceView> {
        let (_, file) = target;
        rg_std::check_cancel!(cancellation, "document source");
        // An opened include may have been discovered after startup. Its captured source owns
        // the saved revision even when module collection has not discovered this file.
        let saved = SavedSourceReader::new(self.query_context());
        if saved.revision(file)? != SourceRevision::from_bytes(source.as_bytes()) {
            return self
                .prepare_current_source(target, source, cancellation)
                .map(DocumentSourceView::Current);
        }
        let line_index = saved.line_index(file)?.clone();
        Ok(DocumentSourceView::SavedExact(line_index))
    }

    /// Associate current declarations with the selected saved file. Other files keep saved semantics.
    pub fn prepare_current_source(
        &self,
        (crate_ref, file): (CrateRef, FileId),
        source: &str,
        cancellation: &rg_std::CancellationToken,
    ) -> anyhow::Result<CurrentSourceView> {
        rg_std::check_cancel!(cancellation, "current source");
        let edition = PackageInput::at(crate_ref.package).edition(self.query_context())?;
        let current_source = Arc::new(CurrentSource::new(Arc::<str>::from(source), edition));
        let current_syntax = current_source.parse().tree();
        let saved = SavedSourceReader::new(self.query_context());
        let saved_syntax = saved
            .syntax(file, edition)
            .context("parse saved syntax for declaration association")?
            .tree();
        let associations = Arc::new(DeclarationAssociationIndex::new(
            &current_syntax,
            &saved_syntax,
        ));
        let relationship = if current_source.revision() == saved.revision(file)? {
            SavedSourceRelationship::Exact
        } else {
            SavedSourceRelationship::Different
        };
        Ok(CurrentSourceView::new(
            current_source,
            crate_ref.package,
            file,
            relationship,
            associations,
        ))
    }

    /// Prepare the selected target's current bodies and declarations without changing saved stores.
    /// The selection keeps cursor recovery and range overlap as separate policies.
    pub fn analysis_for_current_source(
        &self,
        (crate_ref, file): (CrateRef, FileId),
        current_source_view: CurrentSourceView,
        selection: CurrentSourceSelection,
        checkpoint: impl FnMut(CurrentSourceBuildCheckpoint) -> anyhow::Result<()>,
    ) -> anyhow::Result<(Analysis<'a>, CurrentSourceBuildSummary)> {
        let view_db = self.view_db();
        let associations = current_source_view
            .declaration_associations(crate_ref.package, file)
            .context("current-source target has no declaration associations")?;
        let source_changed = current_source_view.relationship(crate_ref.package, file)
            == Some(SavedSourceRelationship::Different);
        let (current, summary) = view_db
            .current_source_builder(current_source_view.source())
            .build_target(
                crate_ref,
                file,
                associations,
                source_changed,
                selection,
                checkpoint,
            )
            .context("prepare current-source target")?;
        let analysis = Analysis::new(view_db.with_current_source(current), self.state.workspace())
            .with_current_source(current_source_view);
        Ok((analysis, summary))
    }
}
