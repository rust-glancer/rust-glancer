//! Selects one crate interpretation for editor queries and exposes saved source text.

use std::{path::Path, sync::Arc};

use anyhow::Context as _;
use rg_def_map::DefMapReadTxn;
use rg_ir_model::{CrateRef, FileId, PackageSlot, Span};
use rg_parse::LineIndex;
#[cfg(test)]
use rg_parse::ParseDb;
use rg_std::MemorySize;
use rg_text::RustEdition;

use super::ProjectSnapshot;
use crate::selection::subset;

/// Analysis-ready context for one filesystem path.
///
/// A shared module can belong to several crates. Editor queries use the first owner in saved
/// package/target order, even when another owner is already materialized or could answer differently.
/// Unreachable parsed files have no semantic context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, MemorySize)]
pub struct FileContext {
    pub crate_ref: CrateRef,
    pub file: FileId,
}

impl<'a> ProjectSnapshot<'a> {
    /// Returns a def-map view over exactly the listed packages, without dependency expansion.
    fn shallow_def_map(&self, packages: &[PackageSlot]) -> DefMapReadTxn<'a> {
        let subset = subset::packages_only(self.state.workspace(), packages);
        self.state.def_map_read_txn_for_subset(&subset)
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

    /// Returns the selected analysis context for a saved filesystem path.
    pub fn file_context_for_path(
        &self,
        path: impl AsRef<Path>,
    ) -> anyhow::Result<Option<FileContext>> {
        let path = path.as_ref();
        let canonical_path = path
            .canonicalize()
            .with_context(|| format!("while attempting to canonicalize {}", path.display()))?;
        self.file_context_for_source_path(&canonical_path)
    }

    /// Returns the selected analysis context for an already-selected project-source identity.
    pub fn file_context_for_source_path(
        &self,
        source_path: &Path,
    ) -> anyhow::Result<Option<FileContext>> {
        let candidates = self.state.file_refs_for_path(source_path);

        let package_slots = candidates
            .iter()
            .map(|file| file.package)
            .collect::<Vec<_>>();
        let def_map = self.shallow_def_map(&package_slots);

        for file in candidates {
            let Some(crate_ref) = def_map
                .crates_for_file(file.package, file.file)
                .context("find crate ownership for source file")?
                .into_iter()
                .next()
            else {
                continue;
            };
            return Ok(Some(FileContext {
                crate_ref,
                file: file.file,
            }));
        }

        Ok(None)
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
