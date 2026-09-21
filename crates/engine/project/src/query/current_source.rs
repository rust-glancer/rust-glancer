//! Prepares captured editor text and request-local bodies against the saved project.

use std::{collections::HashSet, sync::Arc};

use anyhow::Context as _;
use rg_analysis::{Analysis, CurrentSourceView, SavedSourceRelationship, SavedSourceView};
use rg_body_ir::{CurrentSourceBuildCheckpoint, CurrentSourceBuildSummary, CurrentSourceSelection};
use rg_ir_model::{CrateRef, FileId, PackageSlot};
use rg_parse::{CurrentSource, DeclarationAssociationIndex, LineIndex};
use rg_std::UniqueVec;

use super::ProjectSnapshot;
use crate::selection::subset;

/// The source data a document query should use.
///
/// A captured editor document can be identical to the saved file even though it arrived through
/// the editor. In that case the saved line index and saved Body IR already describe the captured
/// bytes, and building another syntax tree would only duplicate work. If any selected saved file
/// differs, the query instead receives one request-local view that contains the captured syntax and
/// its relationship to each saved file interpretation.
#[derive(Debug)]
pub enum DocumentSourceView {
    /// Every selected saved file contains the captured bytes.
    SavedExact(LineIndex),
    /// At least one selected saved file differs from the captured bytes.
    Current(CurrentSourceView),
}

impl DocumentSourceView {
    /// Return the line index belonging to the captured text.
    pub fn line_index(&self) -> &LineIndex {
        match self {
            Self::SavedExact(line_index) => line_index,
            Self::Current(source) => source.source().line_index(),
        }
    }
}

impl<'a> ProjectSnapshot<'a> {
    /// Choose the source data that can safely describe this captured document.
    ///
    /// This checks all crate interpretations because one editor path may represent several saved
    /// file identities. The saved path is allowed only when all of them contain the captured bytes.
    /// Otherwise this prepares one shared current syntax tree and the declaration associations each
    /// interpretation needs.
    pub fn prepare_document_source(
        &self,
        targets: &[(CrateRef, FileId)],
        source: &str,
        cancellation: &rg_std::CancellationToken,
    ) -> anyhow::Result<DocumentSourceView> {
        let &(first_crate, first_file) = targets
            .first()
            .context("document source has no saved file targets")?;

        let source_revision = rg_source::SourceRevision::from_bytes(source.as_bytes());
        for &(crate_ref, file) in targets {
            rg_std::check_cancel!(cancellation, "current source target");
            let saved_file = self
                .state
                .parse_db()
                .package(crate_ref.package.0)
                .context("saved-source target has no parse package")?
                .parsed_file(file)
                .context("saved-source target has no parsed file")?;
            if saved_file.source_revision() != source_revision {
                return self
                    .prepare_current_source(targets, source, cancellation)
                    .map(DocumentSourceView::Current);
            }
        }

        let line_index = self
            .file_line_index(first_crate.package, first_file)
            .context("load exact saved document line index")?
            .context("exact saved document has no line index")?
            .clone();
        Ok(DocumentSourceView::SavedExact(line_index))
    }

    /// Prepare editor text and its relationship to every requested saved file interpretation.
    ///
    /// Text, line indexes, and current syntax are shared. Equality and declaration associations
    /// remain keyed by saved `(package, file)` identity so later query layers do not need to compare
    /// or parse the same sources again.
    pub fn prepare_current_source(
        &self,
        targets: &[(CrateRef, FileId)],
        source: &str,
        cancellation: &rg_std::CancellationToken,
    ) -> anyhow::Result<CurrentSourceView> {
        // `CurrentSource` keys parses by edition, so repeated crate interpretations can be passed
        // through directly without maintaining a second uniqueness policy here.
        let editions = targets
            .iter()
            .map(|(crate_ref, _)| {
                self.state
                    .parse_db()
                    .package(crate_ref.package.0)
                    .context("current-body target has no parse package")
                    .map(rg_parse::Package::edition)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let current_source = Arc::new(CurrentSource::new(Arc::<str>::from(source), editions));
        let mut current_source_view = CurrentSourceView::new(Arc::clone(&current_source));
        let mut prepared_files = HashSet::<(PackageSlot, FileId)>::new();

        for &(crate_ref, file) in targets {
            rg_std::check_cancel!(cancellation, "current source target");
            let key = (crate_ref.package, file);
            if !prepared_files.insert(key) {
                continue;
            }
            let parse_package = self
                .state
                .parse_db()
                .package(crate_ref.package.0)
                .context("current-source target has no parse package")?;
            let current_syntax = current_source
                .parse(parse_package.edition())
                .context("current source was not parsed for this package edition")?
                .tree();
            let saved_file = parse_package
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
            current_source_view.add_saved_interpretation(
                crate_ref.package,
                file,
                relationship,
                associations,
            );
        }

        Ok(current_source_view)
    }

    /// Prepare bodies and selected declarations from this request's captured source.
    ///
    /// The selection keeps cursor recovery and range overlap as separate policies. The callback is
    /// part of every prepared-source build so an interactive request can stop between expensive
    /// phases.
    pub fn analysis_for_current_source(
        &self,
        targets: &[(CrateRef, FileId)],
        current_source_view: CurrentSourceView,
        selection: CurrentSourceSelection,
        cancellation: rg_std::CancellationToken,
        mut checkpoint: impl FnMut(CurrentSourceBuildCheckpoint) -> anyhow::Result<()>,
    ) -> anyhow::Result<(Analysis<'a>, CurrentSourceBuildSummary)> {
        let current_source = current_source_view.source();

        let crates = targets
            .iter()
            .map(|(crate_ref, _)| *crate_ref)
            .collect::<UniqueVec<_>>();
        let subset =
            subset::crates_with_visible_dependencies(self.state.workspace(), crates.as_slice());
        let txn = self
            .state
            .read_txn_for_subset(&subset, cancellation.clone())?;
        let view_db = txn.view_db();
        let mut builder = view_db.current_source_builder(current_source);

        for &(crate_ref, file) in targets {
            rg_std::check_cancel!(cancellation, "current source target");
            let parse_package = self
                .state
                .parse_db()
                .package(crate_ref.package.0)
                .context("current-source target has no parse package")?;
            let associations = current_source_view
                .declaration_associations(crate_ref.package, file)
                .context("current-source target has no declaration associations")?;
            let source_changed = current_source_view.relationship(crate_ref.package, file)
                == Some(SavedSourceRelationship::Different);
            builder
                .prepare_target(
                    parse_package,
                    crate_ref,
                    file,
                    associations,
                    source_changed,
                    selection,
                    view_db
                        .trait_selection(crate_ref)
                        .with_cancellation(cancellation.clone()),
                    &mut checkpoint,
                )
                .context("prepare current-source target")?;
        }

        let (current, summary) = builder
            .finish()
            .context("finish current-source preparation")?;
        let analysis = Analysis::new(
            view_db.clone().with_current_source(current),
            SavedSourceView::new(self.state.parse_db()),
        )
        .with_current_source(current_source_view);
        Ok((analysis, summary))
    }
}
