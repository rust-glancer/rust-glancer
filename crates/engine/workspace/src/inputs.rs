//! Cargo configuration observed by semantic queries.
//!
//! These inputs change when the workspace configuration is refreshed. High durability lets
//! editor requests reuse work that depends only on Cargo facts; a configuration change still
//! invalidates that work.

use rg_cfg_eval::CfgOptions;
use rg_ir_model::{CrateRef, PackageSlot};
use rg_std::NativeOsString;
use rg_text::RustEdition;

use crate::{CargoGeneratedSources, PackageOrigin, TargetKind};

#[ajika::input(key = PackageSlot, durability = High)]
pub struct PackageInput {
    edition: RustEdition,
    #[input(arc)]
    cfg: CfgOptions,
    name: String,
    origin: PackageOrigin,
    #[input(stamp = generated_sources_stamp)]
    generated_sources: Option<CargoGeneratedSources>,
}

#[ajika::input(key = (), durability = High)]
pub struct WorkspaceInput {
    #[input(arc)]
    target_cfg: CfgOptions,
    /// Analyzed targets in package/target order.
    #[input(arc)]
    targets: Vec<CrateRef>,
}

#[ajika::input(key = CrateRef, durability = High)]
pub struct TargetInput {
    root_path: NativeOsString,
    name: String,
    kind: TargetKind,
}

fn generated_sources_stamp(sources: &Option<CargoGeneratedSources>) -> ajika::Result<ajika::Stamp> {
    match sources {
        Some(sources) => sources.stamp(),
        None => Ok(ajika::Stamp::of(&[])),
    }
}
