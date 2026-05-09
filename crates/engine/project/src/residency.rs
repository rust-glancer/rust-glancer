use std::collections::HashSet;

use rg_def_map::PackageSlot;
use rg_workspace::{Package, PackageId, PackageSource, WorkspaceMetadata};

/// Decides which package artifacts should remain resident after a project build.
///
/// This is cache policy, not Cargo metadata. `PackageSource` says where Cargo resolved a package
/// from; residency policy decides how eagerly rust-glancer should keep that package in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PackageResidencyPolicy {
    /// Keep the current pre-cache behavior: all packages stay in memory.
    #[default]
    AllResident,
    /// Keep only workspace packages resident.
    WorkspaceResident,
    /// Keep workspace packages and local path dependencies resident.
    WorkspaceAndPathDepsResident,
    /// Keep workspace packages, local path dependencies, and direct dependencies resident.
    WorkspacePathAndDirectDepsResident,
    /// Treat every package as eligible for offloading.
    AllOffloadable,
}

/// Storage decision for one package in a built project snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackageResidency {
    Resident,
    Offloadable,
}

/// Per-package residency decisions for one workspace metadata snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageResidencyPlan {
    pub(crate) policy: PackageResidencyPolicy,
    pub(crate) packages: Vec<PackageResidency>,
}

impl PackageResidencyPlan {
    pub fn build(workspace: &WorkspaceMetadata, policy: PackageResidencyPolicy) -> Self {
        let direct_dependencies = Self::direct_workspace_dependencies(workspace);
        let packages = workspace
            .packages()
            .iter()
            .map(|package| Self::classify_package(package, policy, &direct_dependencies))
            .collect();

        Self { policy, packages }
    }

    /// Returns the policy that produced this plan.
    pub fn policy(&self) -> PackageResidencyPolicy {
        self.policy
    }

    /// Returns all package decisions in `WorkspaceMetadata::packages()` order.
    pub fn packages(&self) -> &[PackageResidency] {
        &self.packages
    }

    /// Returns one package decision by stable package slot.
    pub fn package(&self, package: PackageSlot) -> Option<PackageResidency> {
        self.packages.get(package.0).copied()
    }

    fn classify_package(
        package: &Package,
        policy: PackageResidencyPolicy,
        direct_dependencies: &HashSet<PackageId>,
    ) -> PackageResidency {
        let is_resident = match policy {
            PackageResidencyPolicy::AllResident => true,
            PackageResidencyPolicy::WorkspaceResident => package.source == PackageSource::Workspace,
            PackageResidencyPolicy::WorkspaceAndPathDepsResident => {
                matches!(
                    package.source,
                    PackageSource::Workspace | PackageSource::Path
                )
            }
            PackageResidencyPolicy::WorkspacePathAndDirectDepsResident => {
                matches!(
                    package.source,
                    PackageSource::Workspace | PackageSource::Path
                ) || direct_dependencies.contains(&package.id)
            }
            PackageResidencyPolicy::AllOffloadable => false,
        };

        if is_resident {
            PackageResidency::Resident
        } else {
            PackageResidency::Offloadable
        }
    }

    fn direct_workspace_dependencies(workspace: &WorkspaceMetadata) -> HashSet<PackageId> {
        workspace
            .workspace_packages()
            .flat_map(|package| package.dependencies.iter())
            .map(|dependency| dependency.package_id().clone())
            .collect()
    }
}

impl PackageResidencyPolicy {
    /// Stable kebab-case name used by CLI flags and LSP initialization options.
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
    use cargo_metadata::Source;
    use test_fixture::fixture_crate;

    use super::{PackageResidency, PackageResidencyPlan, PackageResidencyPolicy};
    use rg_def_map::PackageSlot;
    use rg_workspace::WorkspaceMetadata;

    #[test]
    fn classifies_package_residency_by_policy() {
        let fixture = fixture_crate(
            r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
direct = { path = "direct" }
local = { path = "local" }

//- /src/lib.rs
pub struct App;

//- /direct/Cargo.toml
[package]
name = "direct"
version = "0.1.0"
edition = "2024"

[dependencies]
transitive = { path = "../transitive" }

//- /direct/src/lib.rs
pub struct Direct;

//- /local/Cargo.toml
[package]
name = "local"
version = "0.1.0"
edition = "2024"

//- /local/src/lib.rs
pub struct Local;

//- /transitive/Cargo.toml
[package]
name = "transitive"
version = "0.1.0"
edition = "2024"

//- /transitive/src/lib.rs
pub struct Transitive;
"#,
        );
        let mut metadata = fixture.metadata();
        mark_package_as_registry(&mut metadata, "direct");
        mark_package_as_registry(&mut metadata, "transitive");
        let workspace = WorkspaceMetadata::from_cargo(metadata)
            .expect("fixture workspace metadata should normalize");

        let app = package_slot(&workspace, "app");
        let direct = package_slot(&workspace, "direct");
        let local = package_slot(&workspace, "local");
        let transitive = package_slot(&workspace, "transitive");

        let cases = [
            (
                PackageResidencyPolicy::AllResident,
                [PackageResidency::Resident; 4],
            ),
            (
                PackageResidencyPolicy::WorkspaceResident,
                [
                    PackageResidency::Resident,
                    PackageResidency::Offloadable,
                    PackageResidency::Offloadable,
                    PackageResidency::Offloadable,
                ],
            ),
            (
                PackageResidencyPolicy::WorkspaceAndPathDepsResident,
                [
                    PackageResidency::Resident,
                    PackageResidency::Offloadable,
                    PackageResidency::Resident,
                    PackageResidency::Offloadable,
                ],
            ),
            (
                PackageResidencyPolicy::WorkspacePathAndDirectDepsResident,
                [
                    PackageResidency::Resident,
                    PackageResidency::Resident,
                    PackageResidency::Resident,
                    PackageResidency::Offloadable,
                ],
            ),
            (
                PackageResidencyPolicy::AllOffloadable,
                [PackageResidency::Offloadable; 4],
            ),
        ];

        for (
            policy,
            [
                app_residency,
                direct_residency,
                local_residency,
                transitive_residency,
            ],
        ) in cases
        {
            let plan = PackageResidencyPlan::build(&workspace, policy);
            assert_eq!(plan.package(app), Some(app_residency), "{policy:?} app");
            assert_eq!(
                plan.package(direct),
                Some(direct_residency),
                "{policy:?} direct dependency"
            );
            assert_eq!(
                plan.package(local),
                Some(local_residency),
                "{policy:?} path dependency"
            );
            assert_eq!(
                plan.package(transitive),
                Some(transitive_residency),
                "{policy:?} transitive dependency"
            );
        }
    }

    #[test]
    fn parses_public_policy_names() {
        let cases = [
            ("all-resident", PackageResidencyPolicy::AllResident),
            ("workspace", PackageResidencyPolicy::WorkspaceResident),
            (
                "workspace-and-path-deps",
                PackageResidencyPolicy::WorkspaceAndPathDepsResident,
            ),
            (
                "workspace-path-and-direct-deps",
                PackageResidencyPolicy::WorkspacePathAndDirectDepsResident,
            ),
            ("all-offloadable", PackageResidencyPolicy::AllOffloadable),
        ];

        for (name, policy) in cases {
            assert_eq!(
                PackageResidencyPolicy::from_config_name(name),
                Some(policy),
                "policy name should parse: {name}",
            );
            assert_eq!(policy.config_name(), name);
        }

        assert_eq!(PackageResidencyPolicy::from_config_name("unknown"), None);
    }

    fn mark_package_as_registry(metadata: &mut cargo_metadata::Metadata, name: &str) {
        let package = metadata
            .packages
            .iter_mut()
            .find(|package| package.name == name)
            .unwrap_or_else(|| panic!("package {name} should exist"));
        package.source = Some(Source {
            repr: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
        });
    }

    fn package_slot(workspace: &WorkspaceMetadata, name: &str) -> PackageSlot {
        workspace
            .packages()
            .iter()
            .position(|package| package.name == name)
            .map(PackageSlot)
            .unwrap_or_else(|| panic!("package {name} should exist"))
    }
}
