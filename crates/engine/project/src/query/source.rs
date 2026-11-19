//! Selects one crate interpretation and reads source from the active query input view.
//!
//! File membership comes from declaration outputs. Startup knows module sources; an include
//! reached by lazy member expansion is routed through the same captured source inputs.

use std::path::Path;

use rg_ir_model::{CrateRef, FileId, PackageSlot};
use rg_parse::SourceReader;
use rg_semantic_ir::{complete_source_ownership, target_declarations};
use rg_source::QueryContext;
use rg_std::MemorySize;
use rg_workspace::WorkspaceInput;

use super::ProjectSnapshot;

/// Analysis-ready context for one filesystem path.
///
/// A shared module can belong to several crates. Editor queries prefer a module owner in the
/// file's own Cargo package, then other module owners, before discovering include owners.
/// Within each group, package/target order selects one interpretation.
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

    /// Returns the selected analysis context for an already-selected project-source identity.
    /// Unknown paths require member expansion to discover include owners. Syntax-only requests
    /// can choose an edition with [`crate::Project::source_edition`] without that discovery.
    pub fn file_context_for_source_path(
        &self,
        source_path: &Path,
    ) -> anyhow::Result<Option<FileContext>> {
        let db = self.query_context();
        let file = FileId::for_path(&db.interner(), source_path)?;
        let preferred = self.state.workspace().source_package(source_path);
        let module_crate = match self.saved_sources {
            Some(sources) => sources.first_module_crate(file, preferred),
            None => *first_module_crate(db, file, preferred)?,
        };
        if let Some(crate_ref) = module_crate {
            return Ok(Some(FileContext { crate_ref, file }));
        }
        for (crate_ref, _) in self.state.workspace().targets() {
            let target = target_declarations(db, crate_ref)?;
            if !target
                .source_ownership
                .value_ref(db)?
                .known_owners(file)
                .is_empty()
            {
                return Ok(Some(FileContext { crate_ref, file }));
            }
        }
        for (crate_ref, _) in self.state.workspace().targets() {
            if !complete_source_ownership(db, crate_ref)?
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

/// Start with the package containing the file so a private edit doesn't force declaration
/// rebuilds in dependent crates just to route the request. Check its live targets in order:
/// adding `mod shared;` to a library can give it priority over the binary. Files reached
/// through an external `#[path]` can still find an owner in the rest of the workspace.
#[ajika::query(returns(ref))]
fn first_module_crate(
    db: &QueryContext<'_>,
    file: FileId,
    preferred: Option<PackageSlot>,
) -> ajika::Result<Option<CrateRef>> {
    let targets = WorkspaceInput::at(()).targets(db)?;
    let local = targets
        .iter()
        .filter(|krate| Some(krate.package) == preferred);
    let other = targets
        .iter()
        .filter(|krate| Some(krate.package) != preferred);
    for &krate in local.chain(other) {
        let metadata = target_declarations(db, krate)?.metadata.value_ref(db)?;
        if metadata.sources.modules.binary_search(&file).is_ok() {
            return Ok(Some(krate));
        }
    }
    Ok(None)
}
