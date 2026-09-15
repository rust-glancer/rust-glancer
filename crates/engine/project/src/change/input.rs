//! Saved changes accepted by the project and the summary returned after publication.

use std::path::{Path, PathBuf};

use rg_ir_model::{CrateRef, FileId, PackageSlot};
use rg_source::CapturedSource;
use rg_std::MemorySize;

/// One file change submitted to the saved project.
///
/// `Captured` already contains the Rust text read by the event producer, for example an editor save
/// or a settled file-watcher event. `FsPath` asks the project to inspect the filesystem because the
/// change may add, remove, or rediscover project structure. Both paths are checked against disk
/// before the rebuilt project is published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedFileChange {
    Captured(CapturedSource),
    FsPath(PathBuf),
}

impl SavedFileChange {
    pub fn captured(source: CapturedSource) -> Self {
        Self::Captured(source)
    }

    pub fn fs_path(path: impl AsRef<Path>) -> Self {
        Self::FsPath(path.as_ref().to_path_buf())
    }

    pub fn path(&self) -> &Path {
        match self {
            Self::Captured(source) => source.path(),
            Self::FsPath(path) => path,
        }
    }

    pub fn captured_source(&self) -> Option<&CapturedSource> {
        match self {
            Self::Captured(source) => Some(source),
            Self::FsPath(_) => None,
        }
    }
}

/// Summary of what one saved-file update touched.
#[derive(Debug, Clone, PartialEq, Eq, Default, MemorySize)]
pub struct AnalysisChangeSummary {
    pub changed_files: Vec<ChangedFile>,
    pub affected_packages: Vec<PackageSlot>,
    pub changed_crates: Vec<CrateRef>,
}

impl AnalysisChangeSummary {
    pub(crate) fn is_empty(&self) -> bool {
        self.changed_files.is_empty()
            && self.affected_packages.is_empty()
            && self.changed_crates.is_empty()
    }
}

/// One known package-local source file that was reparsed in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, MemorySize)]
pub struct ChangedFile {
    pub package: PackageSlot,
    pub file: FileId,
}
