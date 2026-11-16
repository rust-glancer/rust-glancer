//! Selects one crate interpretation for editor queries and exposes saved source text.
//!
//! File membership comes from declaration outputs. Startup knows module sources; an include
//! reached by lazy member expansion is routed through the same captured source inputs.

use std::{path::Path, sync::Arc};

use rg_ir_model::{CrateRef, FileId, PackageSlot, Span};
#[cfg(test)]
use rg_parse::ParseDb;
use rg_parse::{LineIndex, SavedSourceReader};
use rg_semantic_ir::{complete_source_ownership, target_declarations};
use rg_std::MemorySize;
use rg_text::RustEdition;

use super::ProjectSnapshot;

/// Analysis-ready context for one filesystem path.
///
/// A shared module can belong to several crates. Editor queries use the first owner in saved
/// package/target order, preferring a known written context before discovering include owners.
/// Exhaustive operations use every owning crate instead of this editor selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, MemorySize)]
pub struct FileContext {
    pub crate_ref: CrateRef,
    pub file: FileId,
}

impl<'a> ProjectSnapshot<'a> {
    #[cfg(test)]
    pub(crate) fn parse_db(&self) -> &'a ParseDb {
        self.state.parse_db()
    }

    /// Convert a source identity even when a lazy member discovered the file after startup.
    pub fn file_path(&self, package: PackageSlot, file: FileId) -> anyhow::Result<Option<&Path>> {
        if self.state.parse_db().package(package.0).is_none() {
            return Ok(None);
        }
        Ok(Some(
            SavedSourceReader::new(self.query_context()).path(file)?,
        ))
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
        if self.state.parse_db().package(package.0).is_none() {
            return Ok(None);
        }
        Ok(SavedSourceReader::new(self.query_context()).text_for_span(file, span)?)
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
        if self.state.parse_db().package(package.0).is_none() {
            return Ok(None);
        }
        Ok(Some(
            SavedSourceReader::new(self.query_context()).text(file)?,
        ))
    }

    /// Returns the line index for a source identity, including files discovered by lazy members.
    pub fn file_line_index(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> anyhow::Result<Option<&LineIndex>> {
        if self.state.parse_db().package(package.0).is_none() {
            return Ok(None);
        }
        Ok(Some(
            SavedSourceReader::new(self.query_context()).line_index(file)?,
        ))
    }

    /// Returns the selected analysis context for an already-selected project-source identity.
    pub fn file_context_for_source_path(
        &self,
        source_path: &Path,
    ) -> anyhow::Result<Option<FileContext>> {
        // Startup's module routing is a cheap positive answer, never proof of absence.
        for file in self.state.parse_db().file_refs_for_path(source_path) {
            if let Some(crate_ref) = self.state.crates_for_file(file.package, file.file).next() {
                return Ok(Some(FileContext {
                    crate_ref,
                    file: file.file,
                }));
            }
        }
        let file = FileId::for_path(&self.query_context().interner(), source_path)?;
        for &crate_ref in &self.state.targets {
            let target = target_declarations(self.query_context(), crate_ref)?;
            if !target
                .source_ownership
                .value_ref(self.query_context())?
                .known_owners(file)
                .is_empty()
            {
                return Ok(Some(FileContext { crate_ref, file }));
            }
        }
        for &crate_ref in &self.state.targets {
            if !complete_source_ownership(self.query_context(), crate_ref)?
                .owners(file)
                .is_empty()
            {
                return Ok(Some(FileContext { crate_ref, file }));
            }
        }
        Ok(None)
    }

    /// Return every target interpretation, including owners discovered through member includes.
    pub fn crates_for_file(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> anyhow::Result<Vec<CrateRef>> {
        let mut crates = Vec::new();
        for crate_ref in self.state.crate_refs_for_package(package) {
            if !complete_source_ownership(self.query_context(), crate_ref)?
                .owners(file)
                .is_empty()
            {
                crates.push(crate_ref);
            }
        }
        Ok(crates)
    }
}
