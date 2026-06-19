use ls_types::LSPAny;
use serde::{Deserialize, Serialize};

use super::section;

/// Protocol-level cache residency policy requested by an LSP client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PackageResidencyPolicy {
    AllResident,
    WorkspaceResident,
    WorkspaceAndPathDepsResident,
    WorkspacePathAndDirectDepsResident,
    AllOffloadable,
}

impl PackageResidencyPolicy {
    pub(super) fn from_initialization_options(options: Option<&LSPAny>) -> Self {
        section(options, "cache")
            .and_then(|cache| cache.get("packageResidency"))
            .and_then(LSPAny::as_str)
            .and_then(Self::from_config_name)
            .unwrap_or(Self::WorkspaceAndPathDepsResident)
    }

    /// Stable kebab-case name accepted in LSP initialization options.
    pub fn config_name(self) -> &'static str {
        match self {
            Self::AllResident => "all-resident",
            Self::WorkspaceResident => "workspace",
            Self::WorkspaceAndPathDepsResident => "workspace-and-path-deps",
            Self::WorkspacePathAndDirectDepsResident => "workspace-path-and-direct-deps",
            Self::AllOffloadable => "all-offloadable",
        }
    }

    /// Parses the public policy names accepted by frontends.
    pub fn from_config_name(value: &str) -> Option<Self> {
        let normalized = value.trim().replace('_', "-").to_ascii_lowercase();
        match normalized.as_str() {
            "all-resident" => Some(Self::AllResident),
            "workspace" | "workspace-resident" => Some(Self::WorkspaceResident),
            "workspace-and-path-deps" | "workspace-path-deps" => {
                Some(Self::WorkspaceAndPathDepsResident)
            }
            "workspace-path-and-direct-deps" | "workspace-path-direct-deps" => {
                Some(Self::WorkspacePathAndDirectDepsResident)
            }
            "all-offloadable" => Some(Self::AllOffloadable),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::PackageResidencyPolicy;

    #[test]
    fn parses_cache_residency_policy() {
        let options = json!({
            "cache": {
                "packageResidency": "all-resident",
            },
        });

        let config = PackageResidencyPolicy::from_initialization_options(Some(&options));

        assert_eq!(config, PackageResidencyPolicy::AllResident);
    }
}
