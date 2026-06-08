use std::{
    collections::HashSet,
    error::Error,
    fmt, io,
    path::{Path, PathBuf},
    process::Command,
};

use rg_cfg_eval::CfgOptions;
use serde::{Deserialize, Serialize};

mod model;
mod sysroot;

#[cfg(test)]
mod tests;

pub use self::{
    model::{
        Package, PackageDependency, PackageId, PackageOrigin, PackageSlot, PackageSource,
        RustEdition, Target, TargetKind, WorkspaceMetadata,
    },
    sysroot::{SysrootCrate, SysrootSources},
};

/// Options used when asking Cargo for the workspace graph.
///
/// Cargo metadata includes dependencies for every platform unless callers pass
/// `--filter-platform`. Analysis wants one concrete graph, so the default resolves the current
/// rustc host triple and lets Cargo prune target-specific dependencies before lowering starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, rg_memsize::MemorySize)]
pub struct CargoMetadataConfig {
    target: CargoMetadataTarget,
}

impl CargoMetadataConfig {
    /// Uses an explicit target triple instead of auto-detecting the rustc host target.
    pub fn target_triple(mut self, target_triple: impl Into<String>) -> Self {
        let target_triple = target_triple.into().trim().to_string();
        self.target = if target_triple.is_empty() {
            CargoMetadataTarget::Auto
        } else {
            CargoMetadataTarget::Triple(target_triple)
        };
        self
    }

    /// Returns the configured target selection before auto-detection is resolved.
    pub fn target(&self) -> &CargoMetadataTarget {
        &self.target
    }

    /// Runs `cargo metadata` with the target-platform filter selected by this configuration.
    pub fn load_metadata(
        &self,
        manifest_path: impl AsRef<Path>,
    ) -> WorkspaceMetadataResult<cargo_metadata::Metadata> {
        let target_triple = self.resolved_target_triple()?;
        self.metadata_command_for_target(manifest_path.as_ref(), &target_triple)?
            .exec()
            .map_err(WorkspaceMetadataError::CargoMetadata)
    }

    /// Runs `cargo metadata --no-deps` and returns canonical workspace member manifests.
    ///
    /// This gives update code a cheap Cargo-backed discovery probe for new workspace members
    /// without constructing a partial analysis graph or resolving third-party dependencies.
    pub fn load_workspace_member_manifest_paths(
        &self,
        manifest_path: impl AsRef<Path>,
    ) -> WorkspaceMetadataResult<Vec<PathBuf>> {
        let metadata = self
            .metadata_command_for_target(manifest_path.as_ref(), &self.resolved_target_triple()?)?
            .no_deps()
            .exec()
            .map_err(WorkspaceMetadataError::CargoMetadata)?;
        let workspace_members = metadata
            .workspace_members
            .iter()
            .map(PackageId::from_cargo)
            .collect::<HashSet<_>>();

        metadata
            .packages
            .into_iter()
            .filter(|package| workspace_members.contains(&PackageId::from_cargo(&package.id)))
            .map(|package| {
                canonicalize_path(package.manifest_path.as_std_path())
                    .map_err(WorkspaceMetadataError::Path)
            })
            .collect()
    }

    fn metadata_command_for_target(
        &self,
        manifest_path: &Path,
        target_triple: &str,
    ) -> WorkspaceMetadataResult<cargo_metadata::MetadataCommand> {
        let mut command = cargo_metadata::MetadataCommand::new();
        command.manifest_path(manifest_path.to_path_buf());
        command.other_options(vec![
            "--filter-platform".to_string(),
            target_triple.to_string(),
        ]);
        Ok(command)
    }

    fn resolved_target_triple(&self) -> WorkspaceMetadataResult<String> {
        match &self.target {
            CargoMetadataTarget::Auto => detect_rustc_host_target(),
            CargoMetadataTarget::Triple(target_triple) => Ok(target_triple.clone()),
        }
    }
}

impl Default for CargoMetadataConfig {
    fn default() -> Self {
        Self {
            target: CargoMetadataTarget::Auto,
        }
    }
}

/// Target platform selection for Cargo metadata filtering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, rg_memsize::MemorySize)]
pub enum CargoMetadataTarget {
    /// Detect the host triple from `rustc -vV`.
    Auto,
    /// Pass this target triple to `cargo metadata --filter-platform`.
    Triple(String),
}

pub type WorkspaceMetadataResult<T> = Result<T, WorkspaceMetadataError>;

#[derive(Debug)]
pub enum WorkspaceMetadataError {
    CargoMetadata(cargo_metadata::Error),
    Path(io::Error),
    RustcHostTarget {
        error: io::Error,
    },
    RustcHostTargetCommandFailed {
        status: String,
        stderr: String,
    },
    RustcHostTargetMissing {
        stdout: String,
    },
    RustcTargetCfg {
        target: String,
        error: io::Error,
    },
    RustcTargetCfgCommandFailed {
        target: String,
        status: String,
        stderr: String,
    },
    UnsupportedPackageSource {
        package: PackageId,
        source: String,
    },
}

impl fmt::Display for WorkspaceMetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CargoMetadata(error) => {
                write!(f, "while attempting to load Cargo metadata: {error}")
            }
            Self::Path(error) => write!(f, "{error}"),
            Self::RustcHostTarget { error } => {
                write!(f, "while attempting to detect rustc host target: {error}")
            }
            Self::RustcHostTargetCommandFailed { status, stderr } => {
                write!(
                    f,
                    "rustc -vV failed while detecting host target: status {status}, stderr: {stderr}"
                )
            }
            Self::RustcHostTargetMissing { stdout } => write!(
                f,
                "rustc -vV output did not contain a host target line: {stdout:?}"
            ),
            Self::RustcTargetCfg { target, error } => {
                write!(
                    f,
                    "while attempting to detect cfg options for {target}: {error}"
                )
            }
            Self::RustcTargetCfgCommandFailed {
                target,
                status,
                stderr,
            } => {
                write!(
                    f,
                    "rustc --print cfg failed for {target}: status {status}, stderr: {stderr}"
                )
            }
            Self::UnsupportedPackageSource { package, source } => write!(
                f,
                "unsupported Cargo source `{source}` for package {package}"
            ),
        }
    }
}

impl Error for WorkspaceMetadataError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CargoMetadata(error) => Some(error),
            Self::Path(error) => Some(error),
            Self::RustcHostTarget { error } => Some(error),
            Self::RustcTargetCfg { error, .. } => Some(error),
            Self::RustcHostTargetCommandFailed { .. }
            | Self::RustcHostTargetMissing { .. }
            | Self::RustcTargetCfgCommandFailed { .. } => None,
            Self::UnsupportedPackageSource { .. } => None,
        }
    }
}

fn detect_rustc_host_target() -> WorkspaceMetadataResult<String> {
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .map_err(|error| WorkspaceMetadataError::RustcHostTarget { error })?;

    if !output.status.success() {
        return Err(WorkspaceMetadataError::RustcHostTargetCommandFailed {
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_rustc_host_target(&stdout).ok_or_else(|| WorkspaceMetadataError::RustcHostTargetMissing {
        stdout: stdout.into_owned(),
    })
}

fn parse_rustc_host_target(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.strip_prefix("host:").map(str::trim))
        .filter(|host| !host.is_empty())
        .map(str::to_string)
}

fn cfg_options_from_rustc_target(target: &str) -> WorkspaceMetadataResult<CfgOptions> {
    let output = Command::new("rustc")
        .args(["--print", "cfg", "--target", target])
        .output()
        .map_err(|error| WorkspaceMetadataError::RustcTargetCfg {
            target: target.to_string(),
            error,
        })?;

    if !output.status.success() {
        return Err(WorkspaceMetadataError::RustcTargetCfgCommandFailed {
            target: target.to_string(),
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    Ok(CfgOptions::from_rustc_cfg_output(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn canonicalize_path(path: &Path) -> io::Result<PathBuf> {
    path.canonicalize().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "while attempting to canonicalize {}: {error}",
                path.display()
            ),
        )
    })
}
