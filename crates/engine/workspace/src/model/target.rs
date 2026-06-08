use std::{
    io,
    path::{Path, PathBuf},
};

use crate::{WorkspaceMetadataError, WorkspaceMetadataResult, path::canonicalize_path};

/// Normalized target metadata with one target kind per target.
#[derive(Debug, Clone, PartialEq, Eq, rg_memsize::MemorySize)]
pub struct Target {
    pub name: String,
    pub kind: TargetKind,
    pub src_path: PathBuf,
}

impl Target {
    pub(crate) fn from_cargo(
        target: &cargo_metadata::Target,
        raw_package_root: &Path,
        package_root: &Path,
        is_workspace_member: bool,
    ) -> WorkspaceMetadataResult<Option<Self>> {
        let Some(src_path) = normalize_target_src_path(
            target.src_path.as_std_path(),
            raw_package_root,
            package_root,
            is_workspace_member,
        )?
        else {
            return Ok(None);
        };

        Ok(Some(Self {
            name: target.name.to_string(),
            kind: TargetKind::from_cargo(target),
            src_path,
        }))
    }
}

fn normalize_target_src_path(
    path: &Path,
    raw_package_root: &Path,
    package_root: &Path,
    is_workspace_member: bool,
) -> WorkspaceMetadataResult<Option<PathBuf>> {
    match canonicalize_path(path) {
        Ok(path) => Ok(Some(path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound && !is_workspace_member => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Keep workspace target identity stable across the edit that declares a target and the
            // later edit that materializes its file. Non-workspace targets do not participate in
            // that save flow, so missing ones are filtered out above.
            let relative_path = path
                .strip_prefix(raw_package_root)
                .map_err(|_| WorkspaceMetadataError::Path(error))?;
            Ok(Some(package_root.join(relative_path)))
        }
        Err(error) => Err(WorkspaceMetadataError::Path(error)),
    }
}

/// Analysis-relevant target kinds.
///
/// We intentionally support less kinds than `cargo_metadata`,
/// since we are only interested in the kinds that are useful
/// for analysis.
#[derive(Debug, Clone, PartialEq, Eq, Hash, derive_more::Display, rg_memsize::MemorySize)]
pub enum TargetKind {
    #[display("lib")]
    Lib,
    #[display("bin")]
    Bin,
    #[display("example")]
    Example,
    #[display("test")]
    Test,
    #[display("bench")]
    Bench,
    #[display("custom-build")]
    CustomBuild,
    #[display("{_0}")]
    Other(String),
}

impl TargetKind {
    pub(crate) fn from_cargo(target: &cargo_metadata::Target) -> Self {
        if target.is_kind(cargo_metadata::TargetKind::Lib) {
            Self::Lib
        } else if target.is_kind(cargo_metadata::TargetKind::Bin) {
            Self::Bin
        } else if target.is_kind(cargo_metadata::TargetKind::Example) {
            Self::Example
        } else if target.is_kind(cargo_metadata::TargetKind::Test) {
            Self::Test
        } else if target.is_kind(cargo_metadata::TargetKind::Bench) {
            Self::Bench
        } else if target.is_kind(cargo_metadata::TargetKind::CustomBuild) {
            Self::CustomBuild
        } else {
            let fallback = target
                .kind
                .first()
                .map(|kind| kind.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            Self::Other(fallback)
        }
    }

    pub fn is_lib(&self) -> bool {
        matches!(self, Self::Lib)
    }

    pub fn is_custom_build(&self) -> bool {
        matches!(self, Self::CustomBuild)
    }

    // Used for predictable ordering, e.g.
    // in test snapshots.
    pub fn sort_order(&self) -> u8 {
        match self {
            Self::Lib => 0,
            Self::Bin => 1,
            Self::Example => 2,
            Self::Test => 3,
            Self::Bench => 4,
            Self::CustomBuild => 5,
            Self::Other(_) => 6,
        }
    }
}
