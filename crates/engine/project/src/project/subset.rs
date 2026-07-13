//! Narrow package subsets.
//!
//! Project residency decides what stays in memory between requests. A subset is narrower: it says
//! which logical packages one query or rebuild may inspect.

use std::collections::HashSet;

use rg_def_map::PackageSlot;
use rg_ir_model::CrateRef;
use rg_package_store::PackageSubset;
use rg_workspace::{PackageId, TargetKind, WorkspaceMetadata};

/// Includes every package in the workspace graph.
pub(crate) fn all(workspace: &WorkspaceMetadata) -> PackageSubset {
    PackageSubset::all(workspace.packages().len())
}

/// Includes the listed package slots without expanding dependency edges.
pub(crate) fn packages_only(
    workspace: &WorkspaceMetadata,
    packages: &[PackageSlot],
) -> PackageSubset {
    let mut subset = empty(workspace);
    for package in packages {
        subset.insert(*package);
    }
    subset
}

/// Includes packages plus every dependency their targets can name during rebuild resolution.
pub(crate) fn rebuild_packages_with_visible_dependencies(
    workspace: &WorkspaceMetadata,
    packages: &[PackageSlot],
) -> PackageSubset {
    let mut subset = empty(workspace);
    let mut expanded = HashSet::new();
    let mut stack = Vec::new();

    for package in packages {
        subset.insert(*package);

        let Some(metadata) = workspace.packages().get(package.0) else {
            continue;
        };
        for target in &metadata.targets {
            if expanded.insert((*package, target.kind.clone())) {
                stack.push((*package, target.kind.clone()));
            }
        }
    }

    expand_visible_dependencies(workspace, &mut subset, &mut expanded, &mut stack);
    subset
}

/// Includes crate packages plus the transitive dependencies visible from those crates.
pub(crate) fn crates_with_visible_dependencies(
    workspace: &WorkspaceMetadata,
    crates: &[CrateRef],
) -> PackageSubset {
    let mut subset = empty(workspace);
    let mut expanded = HashSet::new();
    let mut stack = Vec::new();

    for crate_ref in crates {
        subset.insert(crate_ref.package);

        let Some(target_kind) = crate_target_kind(workspace, *crate_ref) else {
            continue;
        };
        if expanded.insert((crate_ref.package, target_kind.clone())) {
            stack.push((crate_ref.package, target_kind.clone()));
        }
    }

    expand_visible_dependencies(workspace, &mut subset, &mut expanded, &mut stack);
    subset
}

fn expand_visible_dependencies(
    workspace: &WorkspaceMetadata,
    subset: &mut PackageSubset,
    expanded: &mut HashSet<(PackageSlot, TargetKind)>,
    stack: &mut Vec<(PackageSlot, TargetKind)>,
) {
    while let Some((package, target_kind)) = stack.pop() {
        let Some(metadata) = workspace.packages().get(package.0) else {
            continue;
        };

        for dependency in &metadata.dependencies {
            if !dependency.applies_to_target(&target_kind) {
                continue;
            }

            let Some(dependency_slot) = package_slot(workspace, dependency.package_id()) else {
                continue;
            };
            subset.insert(dependency_slot);
            // Dependencies are reached as library crates. Their own dev/build dependencies are not
            // visible to the original crate query.
            if expanded.insert((dependency_slot, TargetKind::Lib)) {
                stack.push((dependency_slot, TargetKind::Lib));
            }
        }
    }
}

fn empty(workspace: &WorkspaceMetadata) -> PackageSubset {
    PackageSubset::empty(workspace.packages().len())
}

/// Returns the Cargo-target kind from which this semantic crate was allocated.
///
/// DefMap assigns `CrateId`s in normalized Cargo-target order. Keeping this positional conversion
/// here makes the project-model boundary explicit instead of letting semantic queries index Cargo
/// metadata directly throughout the engine.
fn crate_target_kind(workspace: &WorkspaceMetadata, crate_ref: CrateRef) -> Option<&TargetKind> {
    workspace
        .packages()
        .get(crate_ref.package.0)?
        .targets
        .get(crate_ref.crate_id.0)
        .map(|target| &target.kind)
}

fn package_slot(workspace: &WorkspaceMetadata, package_id: &PackageId) -> Option<PackageSlot> {
    workspace
        .packages()
        .iter()
        .enumerate()
        .find_map(|(slot, package)| (package.id == *package_id).then_some(PackageSlot(slot)))
}
