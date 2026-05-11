//! Clean def-map finalization.
//!
//! A clean build is the special case of shared finalization where every package is dirty and
//! there is no frozen baseline to read from.

use anyhow::Context as _;

use rg_item_tree::ItemTreeDb;
use rg_text::PackageNameInterners;
use rg_workspace::WorkspaceMetadata;

use super::{
    finalize::{FinalizeTargetStates, finalize_target_states, freeze_package_states},
    implicit_roots::build_implicit_roots,
};
use crate::{DefMapDb, PackageSlot, collect::collect_target_states};

/// Builds the final `DefMapDb` from collected per-target states.
///
/// `collect_target_states` gives us module trees, local definitions, imports, and the initial
/// module scopes that contain only directly declared names. This phase adds the implicit
/// cross-target roots and repeatedly applies imports until the scopes stabilize.
pub(crate) fn build_db(
    workspace: &WorkspaceMetadata,
    parse: &rg_parse::ParseDb,
    item_tree: &ItemTreeDb,
    interners: &mut PackageNameInterners,
) -> anyhow::Result<DefMapDb> {
    // First compute every implicit crate root from the complete package graph. These roots are
    // needed while collecting target states because extern prelude bindings can point across
    // packages and targets.
    let implicit_roots = build_implicit_roots(workspace, parse.packages(), interners)
        .context("while attempting to build implicit target roots")?;

    // A fresh build collects every target from item trees. At this point scopes contain only
    // directly declared names; imports and preludes are deliberately unresolved.
    let target_states =
        collect_target_states(parse.packages(), item_tree, implicit_roots.as_slice())
            .context("while attempting to collect target definitions and imports")?;
    let mut target_states = FinalizeTargetStates::all(target_states);

    finalize_target_states(
        None,
        workspace,
        parse.packages(),
        &mut target_states,
        interners,
    )
    .context("while attempting to finish target states")?;

    let packages = parse
        .packages()
        .iter()
        .enumerate()
        .map(|(package_slot, package)| {
            let package_states = target_states
                .package(PackageSlot(package_slot))
                .expect("clean build should finalize every package");
            freeze_package_states(package, package_states)
        })
        .collect::<Vec<_>>();

    Ok(DefMapDb::from_packages(packages))
}
