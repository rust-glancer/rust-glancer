use ls_types::LSPAny;
use serde::{Deserialize, Serialize};

/// Analysis configuration sent by the LSP client during initialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisConfig {
    pub package_residency_policy: PackageResidencyPolicy,
    pub cargo_metadata_config: CargoMetadataConfig,
}

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

/// Protocol-level Cargo metadata target filter requested by an LSP client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl Default for CargoMetadataConfig {
    fn default() -> Self {
        Self {
            target: CargoMetadataTarget::Auto,
        }
    }
}

/// Target platform selection for Cargo metadata filtering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CargoMetadataTarget {
    Auto,
    Triple(String),
}

impl AnalysisConfig {
    pub fn from_initialization_options(options: Option<&LSPAny>) -> Self {
        let default = Self::default();
        let package_residency_policy = options
            .and_then(LSPAny::as_object)
            .and_then(|options| {
                options
                    .get("cache")
                    .and_then(LSPAny::as_object)
                    .and_then(|cache| cache.get("packageResidency"))
            })
            .and_then(LSPAny::as_str)
            .and_then(PackageResidencyPolicy::from_config_name)
            .unwrap_or(default.package_residency_policy);
        let cargo_metadata_config = options
            .and_then(LSPAny::as_object)
            .and_then(|options| {
                options
                    .get("cargo")
                    .and_then(LSPAny::as_object)
                    .and_then(|cargo| cargo.get("target"))
            })
            .and_then(LSPAny::as_str)
            .map(|target| CargoMetadataConfig::default().target_triple(target))
            .unwrap_or_else(|| default.cargo_metadata_config.clone());

        Self {
            package_residency_policy,
            cargo_metadata_config,
        }
    }
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            // LSP optimizes for low steady-state memory by default. Workspace and local path
            // dependencies are the packages users are most likely to edit by hand, so they remain
            // resident while registry/git dependencies can be offloaded.
            package_residency_policy: PackageResidencyPolicy::WorkspaceAndPathDepsResident,
            cargo_metadata_config: CargoMetadataConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use ls_types::LSPAny;

    use super::{AnalysisConfig, CargoMetadataTarget, PackageResidencyPolicy};

    #[test]
    fn defaults_to_workspace_and_path_dependency_residency() {
        let config = AnalysisConfig::from_initialization_options(None);

        assert_eq!(
            config.package_residency_policy,
            PackageResidencyPolicy::WorkspaceAndPathDepsResident,
        );
        assert_eq!(
            config.cargo_metadata_config.target(),
            &CargoMetadataTarget::Auto
        );
    }

    #[test]
    fn parses_cache_residency_policy() {
        let options = object([(
            "cache",
            object([(
                "packageResidency",
                LSPAny::String("all-resident".to_string()),
            )]),
        )]);

        let config = AnalysisConfig::from_initialization_options(Some(&options));

        assert_eq!(
            config.package_residency_policy,
            PackageResidencyPolicy::AllResident,
        );
    }

    #[test]
    fn parses_cargo_target() {
        let options = object([(
            "cargo",
            object([(
                "target",
                LSPAny::String("x86_64-unknown-linux-gnu".to_string()),
            )]),
        )]);

        let config = AnalysisConfig::from_initialization_options(Some(&options));

        assert_eq!(
            config.cargo_metadata_config.target(),
            &CargoMetadataTarget::Triple("x86_64-unknown-linux-gnu".to_string()),
        );
    }

    fn object<const N: usize>(entries: [(&str, LSPAny); N]) -> LSPAny {
        let mut map = match LSPAny::Object(Default::default()) {
            LSPAny::Object(map) => map,
            _ => unreachable!("constructed object should be an object"),
        };
        for (key, value) in entries {
            map.insert(key.to_string(), value);
        }
        LSPAny::Object(map)
    }
}
