//! Clean def-map finalization.
//!
//! A clean build is the special case of shared finalization where every package is dirty and
//! there is no frozen baseline to read from.

use anyhow::Context as _;

use rg_item_tree::ItemTreeDb;
use rg_macro_runtime::MacroExpansionPerformancePreference;
use rg_text::PackageNameInterners;
use rg_workspace::WorkspaceMetadata;

use super::super::{collect::collect_crate_states, implicit_roots::build_implicit_roots};
use super::{FinalizeCrateStates, finalize_crate_states, freeze_package};
use crate::{DefMapDb, PackageSlot};

/// Builds the final `DefMapDb` from collected per-crate states.
///
/// `collect_crate_states` gives us module trees, local definitions, imports, and the initial
/// module scopes that contain only directly declared names. This phase adds the implicit
/// cross-crate roots and repeatedly applies imports until the scopes stabilize.
pub(crate) fn build_db(
    workspace: &WorkspaceMetadata,
    parse: &rg_parse::ParseDb,
    item_tree: &ItemTreeDb,
    interners: &mut PackageNameInterners,
    performance_preference: MacroExpansionPerformancePreference,
) -> anyhow::Result<DefMapDb> {
    // First compute every implicit crate root from the complete package graph. These roots are
    // needed while collecting crate states because extern prelude bindings can point across
    // packages and crates.
    let implicit_roots = build_implicit_roots(workspace, parse.packages(), interners)
        .context("while attempting to build implicit crate roots")?;

    // A fresh build collects every crate from item trees. At this point scopes contain only
    // directly declared names; imports and preludes are deliberately unresolved.
    let crate_states = collect_crate_states(parse.packages(), item_tree, implicit_roots.as_slice())
        .context("while attempting to collect crate definitions and imports")?;
    let mut crate_states = FinalizeCrateStates::all(crate_states);

    finalize_crate_states(
        None,
        workspace,
        parse.packages(),
        item_tree,
        &mut crate_states,
        interners,
        performance_preference,
        None,
    )
    .context("while attempting to finish crate states")?;

    let packages = parse
        .packages()
        .iter()
        .enumerate()
        .map(|(package_slot, package)| {
            let package_states = crate_states
                .package(PackageSlot(package_slot))
                .expect("clean build should finalize every package");
            freeze_package(package, package_states)
        })
        .collect::<Vec<_>>();

    Ok(DefMapDb::from_packages(packages))
}
