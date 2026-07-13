//! Package-scoped def-map rebuild finalization.
//!
//! Rebuilds collect fresh mutable state only for dirty packages. Shared finalization reads dirty
//! state from that collection and clean state from the previous frozen `DefMapDb`, then this
//! module swaps only rebuilt package payloads into a cloned database.

use anyhow::Context as _;

use rg_item_tree::ItemTreeDb;
use rg_macro_runtime::MacroExpansionPerformancePreference;
use rg_text::PackageNameInterners;
use rg_workspace::WorkspaceMetadata;

use super::super::{collect::collect_package_crate_states, implicit_roots::build_implicit_roots};
use super::{FinalizeCrateStates, finalize_crate_states, freeze_package};
use crate::{DefMapDb, DefMapReadTxn, PackageSlot};

/// Rebuilds selected package def maps against the previous frozen graph.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rebuild_packages(
    old: &DefMapDb,
    old_read: &DefMapReadTxn<'_>,
    workspace: &WorkspaceMetadata,
    parse: &rg_parse::ParseDb,
    item_tree: &ItemTreeDb,
    packages: &[PackageSlot],
    interners: &mut PackageNameInterners,
    performance_preference: MacroExpansionPerformancePreference,
) -> anyhow::Result<DefMapDb> {
    let packages = normalized_package_slots(packages);
    if packages.is_empty() {
        return Ok(old.clone());
    }

    // Implicit roots are still recomputed from metadata even for package-scoped source rebuilds,
    // because the rebuilt crates need the same cross-crate root map shape as a clean build.
    let implicit_roots = build_implicit_roots(workspace, parse.packages(), interners)
        .context("while attempting to rebuild implicit crate roots")?;

    // Only affected packages get mutable state. Unaffected packages remain frozen in `old` and
    // are read through the shared finalization environment.
    let mut crate_states = FinalizeCrateStates::empty(parse.packages().len());

    for package_slot in &packages {
        let parse_package = parse.package(package_slot.0).with_context(|| {
            format!(
                "while attempting to fetch parsed package {}",
                package_slot.0
            )
        })?;
        let item_tree_package = item_tree.package(package_slot.0).with_context(|| {
            format!(
                "while attempting to fetch item-tree package {}",
                package_slot.0
            )
        })?;
        let package_states = collect_package_crate_states(
            package_slot.0,
            parse_package,
            item_tree_package,
            implicit_roots.as_slice(),
        )
        .with_context(|| {
            format!(
                "while attempting to rebuild crate states for package {}",
                parse_package.package_name()
            )
        })?;

        crate_states
            .replace_package(*package_slot, package_states)
            .with_context(|| {
                format!(
                    "while attempting to replace crate states for package {}",
                    package_slot.0
                )
            })?;
    }

    finalize_crate_states(
        Some(old_read),
        workspace,
        parse.packages(),
        item_tree,
        &mut crate_states,
        interners,
        performance_preference,
    )
    .context("while attempting to finish rebuilt crate states")?;

    // Preserve the old snapshot shape and swap in only rebuilt package payloads. This keeps the DB
    // immutable from query consumers' point of view while avoiding a whole-workspace replacement.
    let mut next = old.clone();
    for package_slot in packages {
        let package_states = crate_states.take_package(package_slot).with_context(|| {
            format!(
                "while attempting to fetch rebuilt crate states for package {}",
                package_slot.0
            )
        })?;
        let parse_package = parse.package(package_slot.0).with_context(|| {
            format!(
                "while attempting to fetch parsed package {}",
                package_slot.0
            )
        })?;
        let rebuilt = freeze_package(parse_package, &package_states);
        next.mutator()
            .replace_package(package_slot, rebuilt)
            .with_context(|| {
                format!(
                    "while attempting to replace def-map package {}",
                    package_slot.0
                )
            })?;
    }

    Ok(next)
}

fn normalized_package_slots(packages: &[PackageSlot]) -> Vec<PackageSlot> {
    let mut slots = packages.to_vec();
    slots.sort_by_key(|slot| slot.0);
    slots.dedup();
    slots
}
