use std::path::Path;

use rg_arena::Arena;
use rg_cfg_eval::CfgOptions;
use rg_ir_model::FileId;
use rg_std::MemorySize;
use rg_text::RustEdition;

use crate::{CargoTarget, CargoTargetId, ParsedFile, file::FileDb};

/// Parsed package, including package-local files and target entrypoints.
#[derive(Debug, Clone, MemorySize)]
pub struct Package {
    /// Package name from `Cargo.toml`.
    pub(crate) package_name: String,
    /// Whether this package belongs to the analyzed workspace.
    pub(crate) is_workspace_member: bool,
    /// Active cfg facts inherited from Cargo and the selected target platform.
    pub(crate) cfg_options: CfgOptions,
    /// Manifest feature names retained for request-local `cfg(feature = ...)` completion.
    pub(crate) declared_features: Vec<String>,
    /// All parsed files known to this package.
    pub(crate) files: FileDb,
    /// Parsed targets rooted in this package.
    pub(crate) targets: Arena<CargoTargetId, CargoTarget>,
}

impl Package {
    /// Returns the cached parsed file for a previously known `FileId`.
    pub fn parsed_file(&self, file_id: FileId) -> Option<ParsedFile<'_>> {
        self.files.parsed_file(file_id)
    }

    /// Iterates over all files parsed for this package.
    pub fn parsed_files(&self) -> impl Iterator<Item = ParsedFile<'_>> {
        self.files.parsed_files()
    }

    /// Returns the canonical ID if this package already knows the source path.
    pub(crate) fn file_id_for_path(&self, file_path: &Path) -> Option<FileId> {
        self.files.file_id_for_path(file_path)
    }

    /// Returns the path associated with a file id, if the id is valid.
    pub fn file_path(&self, file_id: FileId) -> Option<&Path> {
        self.files.file_path(file_id)
    }

    /// Returns the logical package name from the parsed package.
    pub fn package_name(&self) -> &str {
        &self.package_name
    }

    /// Returns the Rust edition used to parse this package.
    pub fn edition(&self) -> RustEdition {
        self.files.edition
    }

    /// Returns whether this package belongs to the analyzed workspace.
    pub fn is_workspace_member(&self) -> bool {
        self.is_workspace_member
    }

    /// Returns active package/platform cfg facts.
    pub fn cfg_options(&self) -> &CfgOptions {
        &self.cfg_options
    }

    pub fn declared_features(&self) -> &[String] {
        &self.declared_features
    }

    pub fn targets(&self) -> &[CargoTarget] {
        self.targets.as_slice()
    }

    /// Returns one parsed target by stable id.
    pub fn target(&self, target_id: CargoTargetId) -> Option<&CargoTarget> {
        self.targets.get(target_id)
    }

    /// Assemble the package routing recovered from target manifests.
    pub(super) fn from_parts(
        package: &rg_workspace::Package,
        files: FileDb,
        parsed_targets: Arena<CargoTargetId, CargoTarget>,
    ) -> Self {
        Self {
            package_name: package.name.clone(),
            is_workspace_member: package.is_workspace_member,
            cfg_options: package.cfg_options.clone(),
            declared_features: package.declared_features.clone(),
            files,
            targets: parsed_targets,
        }
    }
}
