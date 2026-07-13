//! Builds the implicit crate roots used by def-map resolution.
//!
//! A root is a textual first path segment that can start resolution outside the current module
//! tree, such as a dependency crate name. An implicit root is not declared by a `mod` item in the
//! current crate; it is injected from package metadata so paths like `serde::Serialize` can begin
//! at the dependency's library root module.
//!
//! Crate collection needs to know which implicit roots are visible before import resolution can
//! start. This pass derives that map from Cargo metadata: sibling crates can see their package
//! library by crate name, and dependencies expose their library target when they apply to the
//! originating Cargo target kind.

use std::collections::HashMap;

use anyhow::Context as _;

use rg_parse::Package;
use rg_text::{Name, PackageNameInterners};
use rg_workspace::WorkspaceMetadata;

use rg_ir_model::{CrateId, CrateRef, DefMapRef, ModuleId, ModuleRef};

use crate::PackageSlot;

/// Implicit roots for one semantic crate.
type CrateImplicitRoots = HashMap<Name, ModuleRef>;

/// Implicit roots for every semantic crate inside one package.
type PackageCrateImplicitRoots = Vec<CrateImplicitRoots>;

/// Implicit crate roots available to each semantic crate.
///
/// The axes mirror DefMap allocation: package slot, then crate slot assigned in normalized Cargo
/// target order, then textual root name. Each root points at the referenced library root module.
pub(super) struct ImplicitRoots {
    package_roots: Vec<PackageCrateImplicitRoots>,
}

impl ImplicitRoots {
    fn new(package_roots: Vec<PackageCrateImplicitRoots>) -> Self {
        Self { package_roots }
    }

    pub(super) fn as_slice(&self) -> &[PackageCrateImplicitRoots] {
        &self.package_roots
    }
}

/// Builds the per-crate root-name map used as the first step of cross-crate resolution.
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
                        (
                            CrateRef {
                                package: PackageSlot(package_slot),
                                crate_id: CrateId(target.id.0),
                            },
                            target.id,
                        ),
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
            let mut crate_roots = HashMap::new();

            // Cargo lets package targets refer to their sibling library by crate name, but build
            // scripts are separate crates and only see explicit build-dependencies.
            if let Some(&(lib_crate, lib_target)) = lib_targets.get(package.id())
                && lib_target != target.id
                && !target.kind.is_custom_build()
            {
                let lib_name = package
                    .target(lib_target)
                    .expect("library target should exist")
                    .name
                    .clone();
                crate_roots.insert(
                    interner.intern(lib_name),
                    ModuleRef {
                        origin: DefMapRef::Crate(lib_crate),
                        module: ModuleId(0),
                    },
                );
            }

            for dependency in &workspace_package.dependencies {
                if dependency.name().is_empty() || !dependency.applies_to_target(&target.kind) {
                    continue;
                }

                let Some(&(lib_crate, _)) = lib_targets.get(dependency.package_id()) else {
                    continue;
                };

                crate_roots.insert(
                    interner.intern(dependency.name()),
                    ModuleRef {
                        origin: DefMapRef::Crate(lib_crate),
                        module: ModuleId(0),
                    },
                );
            }

            package_roots.push(crate_roots);
        }

        roots.push(package_roots);
    }

    Ok(ImplicitRoots::new(roots))
}
