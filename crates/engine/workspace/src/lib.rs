use std::{error::Error, fmt, io};

mod cargo;
mod model;
mod path;
mod rustc;
mod sysroot;

#[cfg(test)]
mod tests;

pub use self::{
    cargo::{CargoMetadataConfig, CargoMetadataTarget},
    model::{
        Package, PackageDependency, PackageId, PackageOrigin, PackageSlot, PackageSource,
        RustEdition, Target, TargetKind, WorkspaceMetadata,
    },
    rustc::RustcTarget,
    sysroot::{SysrootCrate, SysrootSources},
};

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
