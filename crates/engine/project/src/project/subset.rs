//! Narrow package subsets.
//!
//! Project residency decides what stays in memory between requests. A subset is narrower: it says
//! which logical packages one query or rebuild may inspect.

use std::collections::HashSet;

use rg_def_map::PackageSlot;
use rg_ir_model::CrateRef;
use rg_package_store::PackageSubset;
use rg_workspace::{SysrootCrate, TargetKind, WorkspaceMetadata};

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

        // `proc_macro` is a compiler-provided root rather than a Cargo dependency, but later
        // semantic phases still need permission to read its package payload. Mirror DefMap's
        // target boundary here so a proc-macro crate sees the root without leaking it into an
        // ordinary sibling target's query subset.
        if target_kind.is_proc_macro()
            && let Some(proc_macro) = workspace.sysroot_package(SysrootCrate::ProcMacro)
            && let Some(proc_macro_slot) = workspace.package_slot(&proc_macro.id)
        {
            subset.insert(proc_macro_slot);
            if expanded.insert((proc_macro_slot, TargetKind::Lib)) {
                stack.push((proc_macro_slot, TargetKind::Lib));
            }
        }

        for dependency in &metadata.dependencies {
            if !dependency.applies_to_target(&target_kind) {
                continue;
            }

            let Some(dependency_slot) = workspace.package_slot(dependency.package_id()) else {
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

#[cfg(test)]
mod tests {
    use rg_ir_model::CrateId;
    use rg_workspace::{SysrootSources, WorkspaceLoweringConfig};
    use test_fixture::fixture_crate;

    use super::*;

    #[test]
    fn proc_macro_root_participates_only_in_matching_target_subsets() {
        let fixture = fixture_crate(
            r#"
//- /Cargo.toml
[package]
name = "mixed_targets"
version = "0.1.0"
edition = "2024"

[lib]
proc-macro = true

[[bin]]
name = "ordinary"
path = "src/main.rs"

//- /src/lib.rs
use proc_macro::TokenStream;

//- /src/main.rs
fn main() {}
"#,
        )
        .with_fake_sysroot();
        let sysroot = SysrootSources::from_library_root(fixture.path("sysroot/library"))
            .expect("fake sysroot should be complete");
        let workspace =
            WorkspaceMetadata::for_tests(fixture.metadata(), WorkspaceLoweringConfig::default())
                .expect("fixture workspace metadata should build")
                .with_sysroot_sources(Some(sysroot));
        let mixed_slot = workspace
            .packages()
            .iter()
            .position(|package| package.name == "mixed_targets")
            .map(PackageSlot)
            .expect("fixture package should exist");
        let mixed = &workspace.packages()[mixed_slot.0];
        let crate_ref = |kind: TargetKind| CrateRef {
            package: mixed_slot,
            crate_id: CrateId(
                mixed
                    .targets
                    .iter()
                    .position(|target| target.kind == kind)
                    .expect("fixture target should exist"),
            ),
        };
        let proc_macro_slot = workspace
            .sysroot_package(SysrootCrate::ProcMacro)
            .and_then(|package| workspace.package_slot(&package.id))
            .expect("proc_macro package should be modeled");

        let proc_macro =
            crates_with_visible_dependencies(&workspace, &[crate_ref(TargetKind::ProcMacro)]);
        let ordinary = crates_with_visible_dependencies(&workspace, &[crate_ref(TargetKind::Bin)]);
        let package_rebuild = rebuild_packages_with_visible_dependencies(&workspace, &[mixed_slot]);

        assert!(
            proc_macro.contains(proc_macro_slot),
            "proc-macro target queries should include their compiler-provided root",
        );
        assert!(
            !ordinary.contains(proc_macro_slot),
            "ordinary sibling target queries should not include proc_macro",
        );
        assert!(
            package_rebuild.contains(proc_macro_slot),
            "package-wide rebuilds should include roots visible to any rebuilt target",
        );
    }
}
