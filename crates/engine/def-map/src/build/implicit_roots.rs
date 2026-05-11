//! Builds the implicit crate roots used by def-map resolution.
//!
//! A root is a textual first path segment that can start resolution outside the current module
//! tree, such as a dependency crate name. An implicit root is not declared by a `mod` item in the
//! current target; it is injected from package metadata so paths like `serde::Serialize` can begin
//! at the dependency's library root module.
//!
//! Target collection needs to know which implicit roots are visible before import resolution can
//! start. This pass derives that map from Cargo metadata: sibling targets can see their package
//! library by crate name, and dependencies expose their library target when they apply to the
//! current target kind.

use std::collections::HashMap;

use anyhow::Context as _;

use rg_parse::Package;
use rg_text::{Name, PackageNameInterners};
use rg_workspace::WorkspaceMetadata;

use crate::{ModuleId, ModuleRef, PackageSlot, TargetRef};

/// Implicit roots for one target.
type TargetImplicitRoots = HashMap<Name, ModuleRef>;

/// Implicit roots for every target inside one package.
type PackageImplicitRoots = Vec<TargetImplicitRoots>;

/// Implicit crate roots available to each package target.
///
/// The axes mirror the parsed package graph: package slot, then target slot, then textual root
/// name. Each root points at the referenced library root module.
pub(super) struct ImplicitRoots {
    package_roots: Vec<PackageImplicitRoots>,
}

impl ImplicitRoots {
    fn new(package_roots: Vec<PackageImplicitRoots>) -> Self {
        Self { package_roots }
    }

    pub(super) fn as_slice(&self) -> &[PackageImplicitRoots] {
        &self.package_roots
    }
}

/// Builds the per-target root-name map used as the first step of cross-target resolution.
pub(super) fn build_implicit_roots(
    workspace: &WorkspaceMetadata,
    packages: &[Package],
    interners: &mut PackageNameInterners,
) -> anyhow::Result<ImplicitRoots> {
    let lib_targets = packages
        .iter()
        .enumerate()
        .filter_map(|(package_slot, package)| {
            package
                .targets()
                .iter()
                .find(|target| target.kind.is_lib())
                .map(|target| {
                    (
                        package.id().clone(),
                        TargetRef {
                            package: PackageSlot(package_slot),
                            target: target.id,
                        },
                    )
                })
        })
        .collect::<HashMap<_, _>>();
    let mut roots = Vec::with_capacity(packages.len());

    for (package_slot, package) in packages.iter().enumerate() {
        let interner = interners.package_mut(package_slot).with_context(|| {
            format!("while attempting to fetch name interner for package {package_slot}")
        })?;
        let mut package_roots = Vec::with_capacity(package.targets().len());
        let workspace_package = workspace.package(package.id()).with_context(|| {
            format!(
                "while attempting to fetch workspace metadata for package {}",
                package.id()
            )
        })?;

        for target in package.targets() {
            let mut target_roots = HashMap::new();

            // Cargo lets package targets refer to their sibling library by crate name, but build
            // scripts are separate crates and only see explicit build-dependencies.
            if let Some(&lib_target) = lib_targets.get(package.id()) {
                if lib_target.target != target.id && !target.kind.is_custom_build() {
                    let lib_name = package
                        .target(lib_target.target)
                        .expect("library target should exist")
                        .name
                        .clone();
                    target_roots.insert(
                        interner.intern(lib_name),
                        ModuleRef {
                            target: lib_target,
                            module: ModuleId(0),
                        },
                    );
                }
            }

            for dependency in &workspace_package.dependencies {
                if dependency.name().is_empty() || !dependency.applies_to_target(&target.kind) {
                    continue;
                }

                let Some(&lib_target) = lib_targets.get(dependency.package_id()) else {
                    continue;
                };

                target_roots.insert(
                    interner.intern(dependency.name()),
                    ModuleRef {
                        target: lib_target,
                        module: ModuleId(0),
                    },
                );
            }

            package_roots.push(target_roots);
        }

        roots.push(package_roots);
    }

    Ok(ImplicitRoots::new(roots))
}
