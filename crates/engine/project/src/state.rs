//! Metadata published together with one accepted saved-source revision.

use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

use rg_def_map::MacroExpansionLimitReport;
use rg_ir_model::{CrateId, CrateRef, FileId, PackageSlot};
use rg_parse::ParseDb;
use rg_std::MemorySize;
use rg_workspace::WorkspaceMetadata;

use crate::{MacroExpansionLimitBuildSummary, ProjectStats};

/// Identifies one saved version of a [`Project`](crate::Project).
///
/// Only publishing a source or workspace change gives the project a new id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, MemorySize)]
#[memsize(leaf)]
pub struct ProjectGenerationId(u64);

impl ProjectGenerationId {
    pub(crate) fn fresh() -> Self {
        static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
        Self(NEXT_GENERATION.fetch_add(1, Ordering::Relaxed))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

/// Routing stays available between operations. Ajika owns all declaration and body results.
#[derive(Debug, MemorySize)]
pub(crate) struct ProjectState {
    pub(crate) generation_id: ProjectGenerationId,
    pub(crate) workspace: WorkspaceMetadata,
    pub(crate) parse: ParseDb,
    pub(crate) targets: Vec<CrateRef>,
    pub(crate) module_files: HashMap<CrateRef, Vec<FileId>>,
    pub(crate) reports: Vec<MacroExpansionLimitReport>,
    pub(crate) macro_expansion_limit_summary: MacroExpansionLimitBuildSummary,
}

impl ProjectState {
    pub(crate) fn workspace(&self) -> &WorkspaceMetadata {
        &self.workspace
    }

    pub(crate) fn parse_db(&self) -> &ParseDb {
        &self.parse
    }

    pub(crate) fn stats(&self) -> ProjectStats {
        ProjectStats {
            package_count: self.parse.package_count(),
            workspace_package_count: self.parse.workspace_packages().count(),
            target_count: self.module_files.len(),
            file_count: self
                .parse
                .packages()
                .iter()
                .map(|package| package.parsed_files().count())
                .sum(),
        }
    }

    pub(crate) fn crate_refs_for_package(
        &self,
        package: PackageSlot,
    ) -> impl Iterator<Item = CrateRef> + '_ {
        self.parse
            .package(package.0)
            .into_iter()
            .flat_map(move |parsed| {
                parsed.targets().iter().map(move |target| CrateRef {
                    package,
                    crate_id: CrateId(target.id.0),
                })
            })
    }

    pub(crate) fn crates_for_file(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> impl Iterator<Item = CrateRef> + '_ {
        self.crate_refs_for_package(package)
            .filter(move |krate| self.module_files[krate].binary_search(&file).is_ok())
    }
}
