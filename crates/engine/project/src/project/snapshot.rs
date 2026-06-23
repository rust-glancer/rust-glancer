use rg_std::MemorySize;
use std::path::Path;

use anyhow::Context as _;

use rg_analysis::{Analysis, ReferenceSearchFile, ReferenceSearchLabel};
use rg_def_map::{DefMapReadTxn, PackageSlot};
use rg_ir_model::TargetRef;
#[cfg(test)]
use rg_parse::ParseDb;
use rg_parse::{FileId, LineIndex, Span};
use rg_workspace::RustEdition;

use super::{
    FileContext, reference_search::ReferenceSearchPlanner, state::ProjectState,
    stats::ProjectStats, subset,
};

/// Immutable project view used to answer LSP-shaped queries.
#[derive(Debug, Clone, Copy)]
pub struct ProjectSnapshot<'a> {
    pub(super) state: &'a ProjectState,
}

impl<'a> ProjectSnapshot<'a> {
    /// Returns a full-project analysis view.
    pub fn full_analysis(&self) -> anyhow::Result<Analysis<'a>> {
        let txn = self.state.read_txn()?;
        Ok(self.state.analysis(&txn))
    }

    /// Returns an analysis view scoped to the package dependency closure of target queries.
    pub fn analysis_for_targets(&self, targets: &[TargetRef]) -> anyhow::Result<Analysis<'a>> {
        let subset = subset::targets_with_visible_dependencies(self.state.workspace(), targets);
        let txn = self.state.read_txn_for_subset(&subset)?;
        Ok(self.state.analysis(&txn))
    }

    /// Returns a def-map view over exactly the listed packages, without dependency expansion.
    fn shallow_def_map(&self, packages: &[PackageSlot]) -> DefMapReadTxn<'a> {
        let subset = subset::packages_only(self.state.workspace(), packages);
        self.state.def_map_read_txn_for_subset(&subset)
    }

    /// Returns targets whose source should be scanned for an explicit references query.
    ///
    /// Queries scan the selected declaration packages and their package reverse-dependency
    /// closure. Workspace-origin queries keep that closure focused on workspace members, falling
    /// back to the whole workspace only when the declaration package is graph-opaque.
    pub fn reference_search_targets(
        &self,
        origin_package: PackageSlot,
        declaration_targets: &[TargetRef],
    ) -> Vec<TargetRef> {
        ReferenceSearchPlanner::new(self.state).targets(origin_package, declaration_targets)
    }

    /// Returns target/file pairs whose source text contains one of the safe reference labels.
    ///
    /// This is a request-local text prefilter. It narrows expensive semantic scans without storing
    /// a persistent text index or changing the declaration matcher that proves each result.
    pub fn reference_search_files_matching_labels(
        &self,
        search_targets: &[TargetRef],
        labels: &[ReferenceSearchLabel],
    ) -> anyhow::Result<Option<Vec<ReferenceSearchFile>>> {
        ReferenceSearchPlanner::new(self.state).files_matching_labels(search_targets, labels)
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
    ) -> Option<String> {
        self.state
            .parse_db()
            .package(package.0)?
            .parsed_file(file)?
            .text_for_span(span)
    }

    /// Returns the line index used to convert offsets for a package-local file id.
    pub fn file_line_index(&self, package: PackageSlot, file: FileId) -> Option<&LineIndex> {
        self.state
            .parse_db()
            .package(package.0)?
            .parsed_file(file)
            .and_then(|file| file.line_index().ok())
    }

    pub fn stats(&self) -> ProjectStats {
        self.state.stats()
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
        let canonical_path = path
            .canonicalize()
            .with_context(|| format!("while attempting to canonicalize {}", path.display()))?;
        let candidates = self.state.file_refs_for_path(&canonical_path);

        let package_slots = candidates
            .iter()
            .map(|file| file.package)
            .collect::<Vec<_>>();
        let def_map = self.shallow_def_map(&package_slots);
        let mut contexts = Vec::new();

        for file in candidates {
            let targets = def_map
                .targets_for_file(file.package, file.file)
                .context("while attempting to find target ownership for source file")?;
            if targets.is_empty() {
                continue;
            }

            contexts.push(FileContext {
                package: file.package,
                file: file.file,
                targets,
            });
        }

        Ok(contexts)
    }

    /// Returns target contexts whose module tree contains a package-local file.
    pub fn targets_for_file(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> anyhow::Result<Vec<TargetRef>> {
        let def_map = self.shallow_def_map(&[package]);
        def_map
            .targets_for_file(package, file)
            .context("while attempting to find target ownership for source file")
    }
}
