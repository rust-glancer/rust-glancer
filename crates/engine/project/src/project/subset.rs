//! Narrow package subsets.
//!
//! Project residency decides what stays in memory between requests. A subset is narrower: it says
//! which logical packages one query or rebuild may inspect.

use std::collections::HashSet;

use rg_def_map::{PackageSlot, TargetRef};
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

/// Includes target packages plus the transitive dependencies visible from those targets.
pub(crate) fn targets_with_visible_dependencies(
    workspace: &WorkspaceMetadata,
    targets: &[TargetRef],
) -> PackageSubset {
    let mut subset = empty(workspace);
    let mut expanded = HashSet::new();
    let mut stack = Vec::new();

    for target in targets {
        subset.insert(target.package);

        let Some(target_kind) = target_kind(workspace, *target) else {
            continue;
        };
        if expanded.insert((target.package, target_kind.clone())) {
            stack.push((target.package, target_kind.clone()));
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
            // visible to the original target query.
            if expanded.insert((dependency_slot, TargetKind::Lib)) {
                stack.push((dependency_slot, TargetKind::Lib));
            }
        }
    }
}

fn empty(workspace: &WorkspaceMetadata) -> PackageSubset {
    PackageSubset::empty(workspace.packages().len())
}

fn target_kind(workspace: &WorkspaceMetadata, target: TargetRef) -> Option<&TargetKind> {
    workspace
        .packages()
        .get(target.package.0)?
        .targets
        .get(target.target.0)
        .map(|target| &target.kind)
}

fn package_slot(workspace: &WorkspaceMetadata, package_id: &PackageId) -> Option<PackageSlot> {
    workspace
        .packages()
        .iter()
        .enumerate()
        .find_map(|(slot, package)| (package.id == *package_id).then_some(PackageSlot(slot)))
}
