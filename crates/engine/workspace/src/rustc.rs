use std::process::Command;

use rg_cfg_eval::CfgOptions;
use serde::{Deserialize, Serialize};

use crate::{WorkspaceMetadataError, WorkspaceMetadataResult};
use rg_std::MemorySize;

/// Concrete rustc target triple used for Cargo filtering and cfg probing.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, derive_more::Display, Serialize, Deserialize, MemorySize,
)]
#[display("{_0}")]
pub struct RustcTarget(#[memsize(inline)] String);

impl RustcTarget {
    /// Builds a target triple after applying the same trimming used by user-facing config.
    pub fn new(target_triple: impl Into<String>) -> Option<Self> {
        let target_triple = target_triple.into().trim().to_string();
        (!target_triple.is_empty()).then_some(Self(target_triple))
    }

    /// Detects the host triple from `rustc -vV`.
    pub(crate) fn detect_host() -> WorkspaceMetadataResult<Self> {
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
        Self::parse_host_from_verbose_output(&stdout).ok_or_else(|| {
            WorkspaceMetadataError::RustcHostTargetMissing {
                stdout: stdout.into_owned(),
            }
        })
    }

    /// Returns cfg options for this concrete target triple.
    pub(crate) fn cfg_options(&self) -> WorkspaceMetadataResult<CfgOptions> {
        let output = Command::new("rustc")
            .args(["--print", "cfg", "--target", self.as_str()])
            .output()
            .map_err(|error| WorkspaceMetadataError::RustcTargetCfg {
                target: self.as_str().to_string(),
                error,
            })?;

        if !output.status.success() {
            return Err(WorkspaceMetadataError::RustcTargetCfgCommandFailed {
                target: self.as_str().to_string(),
                status: output.status.to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }

        Ok(CfgOptions::from_rustc_cfg_output(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn parse_host_from_verbose_output(output: &str) -> Option<Self> {
        output
            .lines()
            .find_map(|line| line.strip_prefix("host:").map(str::trim))
            .and_then(Self::new)
    }
}
