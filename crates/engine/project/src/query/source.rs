//! Selects one crate interpretation and reads source from the active query input view.
//!
//! File membership comes from declaration outputs. Startup knows module sources; an include
//! reached by lazy member expansion is routed through the same captured source inputs.

use std::path::Path;

use rg_ir_model::{CrateRef, FileId, PackageSlot};
use rg_parse::SourceReader;
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
    /// Physical source access uses only the file identity. A file can have several crate
    /// interpretations; selecting one belongs to routing and semantic queries, not text reads.
    pub fn sources(&self) -> SourceReader<'a> {
        SourceReader::new(self.query_context())
    }

    /// Returns whether a package belongs to the analyzed workspace.
    pub fn package_is_workspace_member(&self, package: PackageSlot) -> bool {
        self.state
            .workspace()
            .packages()
            .get(package.0)
            .is_some_and(|package| package.is_workspace_member)
    }

    /// Returns the Rust edition declared for a package in the current workspace metadata.
    pub fn package_edition(&self, package: PackageSlot) -> Option<RustEdition> {
        self.state
            .workspace()
            .packages()
            .get(package.0)
            .map(|package| package.edition)
    }

    /// Returns the selected analysis context for an already-selected project-source identity.
    pub fn file_context_for_source_path(
        &self,
        source_path: &Path,
    ) -> anyhow::Result<Option<FileContext>> {
        let file = FileId::for_path(&self.query_context().interner(), source_path)?;
        if let Some(crate_ref) = self.state.sources.first_module_crate(file) {
            // Saved routing is a cheap candidate. An editor view may have removed or moved this
            // module, so verify membership against the declarations of the active input revision.
            let metadata = target_declarations(self.query_context(), crate_ref)?
                .metadata
                .value_ref(self.query_context())?;
            if metadata.sources.modules.binary_search(&file).is_ok() {
                return Ok(Some(FileContext { crate_ref, file }));
            }
        }
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
        for (crate_ref, _) in self.state.workspace().targets_for_package(package) {
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
