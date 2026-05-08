use std::path::Path;

use anyhow::Context as _;

use rg_analysis::Analysis;
use rg_def_map::{PackageSlot, TargetRef};
#[cfg(test)]
use rg_parse::ParseDb;
use rg_parse::{FileId, LineIndex};

use super::{FileContext, state::ProjectState, stats::ProjectStats, subset};

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

    /// Returns an analysis view over exactly the listed packages, without dependency expansion.
    ///
    /// This is only suitable for package-local metadata queries such as target/file ownership.
    /// Semantic queries should use a target-scoped analysis so dependencies are visible too.
    pub(crate) fn shallow_analysis(
        &self,
        packages: &[PackageSlot],
    ) -> anyhow::Result<Analysis<'a>> {
        let subset = subset::packages_only(self.state.workspace(), packages);
        let txn = self.state.read_txn_for_subset(&subset)?;
        Ok(self.state.analysis(&txn))
    }

    #[cfg(test)]
    pub(crate) fn parse_db(&self) -> &'a ParseDb {
        self.state.parse_db()
    }

    /// Returns the source path for a package-local file id.
    pub fn file_path(&self, package: PackageSlot, file: FileId) -> Option<&Path> {
        self.state.parse_db().package(package.0)?.file_path(file)
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
        use rg_memsize::MemorySize as _;

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
        let analysis = self.shallow_analysis(&package_slots)?;
        let mut contexts = Vec::new();

        for file in candidates {
            let targets = analysis.targets_for_file(file.package, file.file)?;
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
        let analysis = self.shallow_analysis(&[package])?;
        analysis.targets_for_file(package, file)
    }
}
