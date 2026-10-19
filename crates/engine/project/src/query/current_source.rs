//! Prepares captured editor text and request-local bodies against the saved project.

use std::sync::Arc;

use anyhow::Context as _;
use rg_analysis::{Analysis, CurrentSourceView, SavedSourceRelationship, SavedSourceView};
use rg_body_ir::{CurrentSourceBuildCheckpoint, CurrentSourceBuildSummary, CurrentSourceSelection};
use rg_ir_model::{CrateRef, FileId};
use rg_parse::{CurrentSource, DeclarationAssociationIndex, LineIndex};

use super::ProjectSnapshot;
use crate::selection::subset;

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
        let (crate_ref, file) = target;
        rg_std::check_cancel!(cancellation, "document source");
        let saved_file = self
            .state
            .parse_db()
            .package(crate_ref.package.0)
            .context("saved-source target has no parse package")?
            .parsed_file(file)
            .context("saved-source target has no parsed file")?;
        if saved_file.source_revision() != rg_source::SourceRevision::from_bytes(source.as_bytes())
        {
            return self
                .prepare_current_source(target, source, cancellation)
                .map(DocumentSourceView::Current);
        }
        let line_index = self
            .file_line_index(crate_ref.package, file)?
            .context("exact saved document has no line index")?
            .clone();
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
        let package = self
            .state
            .parse_db()
            .package(crate_ref.package.0)
            .context("current-source target has no parse package")?;
        let current_source = Arc::new(CurrentSource::new(
            Arc::<str>::from(source),
            package.edition(),
        ));
        let current_syntax = current_source.parse().tree();
        let saved_file = package
            .parsed_file(file)
            .context("current-source target has no saved parse file")?;
        let saved_syntax = saved_file
            .parse_syntax()
            .context("parse saved syntax for declaration association")?
            .tree();
        let associations = Arc::new(DeclarationAssociationIndex::new(
            &current_syntax,
            &saved_syntax,
        ));
        let relationship = if current_source.revision() == saved_file.source_revision() {
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
        cancellation: rg_std::CancellationToken,
        checkpoint: impl FnMut(CurrentSourceBuildCheckpoint) -> anyhow::Result<()>,
    ) -> anyhow::Result<(Analysis<'a>, CurrentSourceBuildSummary)> {
        let subset = subset::crates_with_visible_dependencies(self.state.workspace(), &[crate_ref]);
        let txn = self
            .state
            .read_txn_for_subset(&subset, cancellation.clone())?;
        let view_db = txn.view_db();
        let package = self
            .state
            .parse_db()
            .package(crate_ref.package.0)
            .context("current-source target has no parse package")?;
        let associations = current_source_view
            .declaration_associations(crate_ref.package, file)
            .context("current-source target has no declaration associations")?;
        let source_changed = current_source_view.relationship(crate_ref.package, file)
            == Some(SavedSourceRelationship::Different);
        let (current, summary) = view_db
            .current_source_builder(current_source_view.source())
            .build_target(
                package,
                crate_ref,
                file,
                associations,
                source_changed,
                selection,
                checkpoint,
            )
            .context("prepare current-source target")?;
        let analysis = Analysis::new(
            view_db.clone().with_current_source(current),
            SavedSourceView::new(self.state.parse_db()),
        )
        .with_current_source(current_source_view);
        Ok((analysis, summary))
    }
}
