//! Read-only access to one published project generation.
//!
//! Queries use this type instead of opening the individual phase databases themselves. An editor
//! route has already selected the path used by this project, so document queries can use that path
//! directly. Only callers that start with an ordinary filesystem path need canonicalization here.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};

use anyhow::Context as _;

use rg_analysis::{
    Analysis, CurrentSourceView, ReferenceSearchFile, ReferenceSearchLabel,
    SavedSourceRelationship, SavedSourceView,
};
use rg_body_ir::{
    CurrentBodyBuildCheckpoint, CurrentBodyBuildOutcome, CurrentBodySelection, CurrentBodySet,
    CurrentBodyUnavailable,
};
use rg_def_map::{DefMapReadTxn, PackageSlot};
use rg_ir_model::{BodyId, BodyRef, CrateRef};
#[cfg(test)]
use rg_parse::ParseDb;
use rg_parse::{CurrentSource, DeclarationAssociationIndex, FileId, LineIndex, Span};
use rg_std::{MemorySize, UniqueVec};
use rg_text::RustEdition;

use super::{
    FileContext,
    reference_search::ReferenceSearchPlanner,
    state::ProjectState,
    stats::{MacroExpansionLimitBuildSummary, ProjectStats},
    subset,
};

/// Immutable project view used to answer LSP-shaped queries.
#[derive(Debug, Clone, Copy)]
pub struct ProjectSnapshot<'a> {
    pub(super) state: &'a ProjectState,
}

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

/// Describes what happened while current bodies were built for one request.
///
/// One editor path may belong to several crate contexts, and each context is built separately. The
/// returned analysis already contains every body that succeeded. This summary lets diagnostics and
/// tests see whether another context was unavailable and which source spans were rebuilt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentBodyBuildSummary {
    unavailable: Vec<(CrateRef, CurrentBodyUnavailable)>,
    rebuilt_body_spans: Vec<(CrateRef, FileId, Span)>,
}

impl CurrentBodyBuildSummary {
    /// Returns whether body selection and rebuilding succeeded in every requested crate context.
    pub fn is_complete(&self) -> bool {
        self.unavailable.is_empty()
    }

    /// Source spans of bodies that were rebuilt from the editor text.
    pub fn rebuilt_body_spans(&self) -> &[(CrateRef, FileId, Span)] {
        &self.rebuilt_body_spans
    }

    #[cfg(test)]
    pub(crate) fn unavailable(&self) -> &[(CrateRef, CurrentBodyUnavailable)] {
        &self.unavailable
    }
}

impl<'a> ProjectSnapshot<'a> {
    /// Returns a full-project analysis view.
    pub fn full_analysis(&self) -> anyhow::Result<Analysis<'a>> {
        let txn = self.state.read_txn()?;
        Ok(self.state.analysis(&txn))
    }

    /// Returns an analysis view scoped to the package dependency closure of crate queries.
    pub fn analysis_for_crates(&self, crates: &[CrateRef]) -> anyhow::Result<Analysis<'a>> {
        let subset = subset::crates_with_visible_dependencies(self.state.workspace(), crates);
        let txn = self.state.read_txn_for_subset(&subset)?;
        Ok(self.state.analysis(&txn))
    }

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
    ) -> anyhow::Result<DocumentSourceView> {
        let &(first_crate, first_file) = targets
            .first()
            .context("document source has no saved file targets")?;

        let source_revision = rg_source::SourceRevision::from_bytes(source.as_bytes());
        for &(crate_ref, file) in targets {
            let saved_file = self
                .state
                .parse_db()
                .package(crate_ref.package.0)
                .context("saved-source target has no parse package")?
                .parsed_file(file)
                .context("saved-source target has no parsed file")?;
            if saved_file.source_revision() != source_revision {
                return self
                    .prepare_current_source(targets, source)
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

    /// Rebuild the body at `offset` for every requested crate context, checking for cancellation after
    /// each build phase.
    ///
    /// The returned `Analysis` borrows declarations, traits, and impls from this saved project, but
    /// uses the supplied source for any body that can be matched to a saved owner. A body that
    /// cannot be matched is left out; this method never creates a second unsaved project.
    pub fn analysis_for_current_bodies_at_offset(
        &self,
        targets: &[(CrateRef, FileId)],
        source: &str,
        offset: u32,
        checkpoint: impl FnMut(CurrentBodyBuildCheckpoint) -> anyhow::Result<()>,
    ) -> anyhow::Result<(Analysis<'a>, CurrentBodyBuildSummary)> {
        let source = self.prepare_current_source(targets, source)?;
        self.analysis_for_current_bodies_from_source(
            targets,
            source,
            CurrentBodySelection::AtOffset(offset),
            checkpoint,
        )
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

    /// Build current Body IR from source data already prepared for this request.
    ///
    /// The selection keeps cursor recovery and range overlap as separate policies. The callback is
    /// part of every prepared-source build so an interactive request can stop between expensive
    /// phases.
    pub fn analysis_for_current_bodies_from_source(
        &self,
        targets: &[(CrateRef, FileId)],
        current_source_view: CurrentSourceView,
        selection: CurrentBodySelection,
        mut checkpoint: impl FnMut(CurrentBodyBuildCheckpoint) -> anyhow::Result<()>,
    ) -> anyhow::Result<(Analysis<'a>, CurrentBodyBuildSummary)> {
        let current_source = current_source_view.source();

        let crates = targets
            .iter()
            .map(|(crate_ref, _)| *crate_ref)
            .collect::<UniqueVec<_>>();
        let subset =
            subset::crates_with_visible_dependencies(self.state.workspace(), crates.as_slice());
        let txn = self.state.read_txn_for_subset(&subset)?;
        let view_db = txn.view_db();
        let mut bodies = Vec::new();
        let mut unavailable = Vec::new();
        let mut rebuilt_body_spans = Vec::new();
        let mut masked_files = HashSet::new();
        let mut next_synthetic_body_ids = HashMap::<CrateRef, usize>::new();

        for &(crate_ref, file) in targets {
            let parse_package = self
                .state
                .parse_db()
                .package(crate_ref.package.0)
                .context("current-body target has no parse package")?;
            let associations = current_source_view
                .declaration_associations(crate_ref.package, file)
                .context("current-body target has no declaration associations")?;
            if current_source_view.relationship(crate_ref.package, file)
                == Some(SavedSourceRelationship::Different)
            {
                masked_files.insert((crate_ref, file));
            }

            let mut synthetic_body_ref = || {
                let body_ref = match next_synthetic_body_ids.entry(crate_ref) {
                    std::collections::hash_map::Entry::Occupied(mut entry) => {
                        let body = *entry.get();
                        *entry.get_mut() = body
                            .checked_add(1)
                            .context("request-only body identity overflowed")?;
                        BodyRef {
                            crate_ref,
                            body: BodyId(body),
                        }
                    }
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let first = view_db
                            .first_synthetic_body_ref(crate_ref)
                            .context("allocate first request-only body identity")?;
                        entry.insert(
                            first
                                .body
                                .0
                                .checked_add(1)
                                .context("request-only body identity overflowed")?,
                        );
                        first
                    }
                };
                Ok(body_ref)
            };
            let CurrentBodyBuildOutcome {
                bodies: built_bodies,
                unavailable: body_unavailable,
            } = view_db.build_current_bodies(
                parse_package,
                crate_ref,
                file,
                current_source,
                associations,
                selection,
                &mut synthetic_body_ref,
                &mut checkpoint,
            )?;
            for body in built_bodies {
                let source_span = body.source_span();
                rebuilt_body_spans.push((crate_ref, file, source_span));
                bodies.push(body);
            }
            unavailable.extend(
                body_unavailable
                    .into_iter()
                    .map(|reason| (crate_ref, reason)),
            );
        }

        let current = CurrentBodySet::new(masked_files, bodies)
            .context("assemble current bodies for the request")?;
        let view_db = txn.view_db().clone().with_current_body_set(current);
        let analysis = Analysis::new(view_db, SavedSourceView::new(self.state.parse_db()))
            .with_current_source(current_source_view);
        Ok((
            analysis,
            CurrentBodyBuildSummary {
                unavailable,
                rebuilt_body_spans,
            },
        ))
    }

    /// Returns a def-map view over exactly the listed packages, without dependency expansion.
    fn shallow_def_map(&self, packages: &[PackageSlot]) -> DefMapReadTxn<'a> {
        let subset = subset::packages_only(self.state.workspace(), packages);
        self.state.def_map_read_txn_for_subset(&subset)
    }

    /// Returns crates whose source should be scanned for an explicit references query.
    ///
    /// Queries scan the selected declaration packages and their package reverse-dependency
    /// closure. Workspace-origin queries keep that closure focused on workspace members, falling
    /// back to the whole workspace only when the declaration package is graph-opaque.
    pub fn reference_search_crates(
        &self,
        origin_package: PackageSlot,
        declaration_crates: &[CrateRef],
    ) -> Vec<CrateRef> {
        ReferenceSearchPlanner::new(self.state).crates(origin_package, declaration_crates)
    }

    /// Returns crate/file pairs whose source text contains one of the safe reference labels.
    ///
    /// This is a request-local text prefilter. It narrows expensive semantic scans without storing
    /// a persistent text index or changing the declaration matcher that proves each result.
    pub fn reference_search_files_matching_labels(
        &self,
        search_crates: &[CrateRef],
        labels: &[ReferenceSearchLabel],
    ) -> anyhow::Result<Option<Vec<ReferenceSearchFile>>> {
        ReferenceSearchPlanner::new(self.state).files_matching_labels(search_crates, labels)
    }

    #[cfg(test)]
    pub(crate) fn parse_db(&self) -> &'a ParseDb {
        self.state.parse_db()
    }

    /// Returns the source path for a package-local file id.
    pub fn file_path(&self, package: PackageSlot, file: FileId) -> Option<&Path> {
        self.state.parse_db().package(package.0)?.file_path(file)
    }

    /// Returns whether a package belongs to the analyzed workspace.
    pub fn package_is_workspace_member(&self, package: PackageSlot) -> bool {
        self.state
            .parse_db()
            .package(package.0)
            .is_some_and(|package| package.is_workspace_member())
    }

    /// Returns the Rust edition declared for a package in the current workspace metadata.
    pub fn package_edition(&self, package: PackageSlot) -> Option<RustEdition> {
        self.state
            .workspace()
            .packages()
            .get(package.0)
            .map(|package| package.edition)
    }

    /// Returns source text for a byte span from the same snapshot that backs this project view.
    pub fn file_text_for_span(
        &self,
        package: PackageSlot,
        file: FileId,
        span: Span,
    ) -> anyhow::Result<Option<String>> {
        let Some(parsed_file) = self
            .state
            .parse_db()
            .package(package.0)
            .and_then(|package| package.parsed_file(file))
        else {
            return Ok(None);
        };
        parsed_file.text_for_span(span)
    }

    /// Returns request-scoped source text for syntax-sensitive editor queries.
    ///
    /// Saved text may have been evicted after indexing; loading it here does not retain it in the
    /// project graph once the returned `Arc` and query-local source handle are dropped.
    pub fn file_source_text(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> anyhow::Result<Option<Arc<str>>> {
        let Some(parsed_file) = self
            .state
            .parse_db()
            .package(package.0)
            .and_then(|package| package.parsed_file(file))
        else {
            return Ok(None);
        };
        Ok(Some(
            parsed_file
                .source_text()
                .context("load parsed file source")?,
        ))
    }

    /// Returns the line index used to convert offsets for a package-local file id.
    pub fn file_line_index(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> anyhow::Result<Option<&LineIndex>> {
        let Some(parsed_file) = self
            .state
            .parse_db()
            .package(package.0)
            .and_then(|package| package.parsed_file(file))
        else {
            return Ok(None);
        };
        Ok(Some(parsed_file.line_index()?))
    }

    pub fn stats(&self) -> ProjectStats {
        self.state.stats()
    }

    /// Returns bounded diagnostics from the def-map packages built for this project state.
    pub fn macro_expansion_limit_summary(&self) -> &MacroExpansionLimitBuildSummary {
        &self.state.macro_expansion_limit_summary
    }

    /// Returns an approximate retained-memory total for the current immutable analysis graph.
    ///
    /// This is intended for observability, not correctness. Computing it walks the graph, so LSP
    /// callers should keep it behind explicit memory logging.
    pub fn retained_memory_bytes(&self) -> usize {
        use MemorySize as _;

        self.state.memory_size()
    }

    /// Returns current analysis contexts for a saved filesystem path.
    pub fn file_contexts_for_path(
        &self,
        path: impl AsRef<Path>,
    ) -> anyhow::Result<Vec<FileContext>> {
        let path = path.as_ref();
        let canonical_path = rg_std::path::canonicalize(path)
            .with_context(|| format!("while attempting to canonicalize {}", path.display()))?;
        self.file_contexts_for_source_path(&canonical_path)
    }

    /// Returns current analysis contexts for an already-selected project-source identity.
    pub fn file_contexts_for_source_path(
        &self,
        source_path: &Path,
    ) -> anyhow::Result<Vec<FileContext>> {
        let candidates = self.state.file_refs_for_path(source_path);

        let package_slots = candidates
            .iter()
            .map(|file| file.package)
            .collect::<Vec<_>>();
        let def_map = self.shallow_def_map(&package_slots);
        let mut contexts = Vec::new();

        for file in candidates {
            let crates = def_map
                .crates_for_file(file.package, file.file)
                .context("while attempting to find crate ownership for source file")?;
            if crates.is_empty() {
                continue;
            }

            contexts.push(FileContext {
                package: file.package,
                file: file.file,
                crates,
            });
        }

        Ok(contexts)
    }

    /// Returns crate contexts whose module tree contains a package-local file.
    pub fn crates_for_file(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> anyhow::Result<Vec<CrateRef>> {
        let def_map = self.shallow_def_map(&[package]);
        def_map
            .crates_for_file(package, file)
            .context("while attempting to find crate ownership for source file")
    }
}
