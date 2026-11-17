use rg_cfg_eval::CfgOptions;
use rg_ir_model::{CrateRef, PackageSlot};
use rg_std::NativeOsString;
use rg_text::RustEdition;

use crate::{CargoGeneratedSources, PackageOrigin, TargetKind};

#[ajika::input(key = PackageSlot)]
pub struct PackageInput {
    edition: RustEdition,
    #[input(arc)]
    cfg: CfgOptions,
    name: String,
    origin: PackageOrigin,
    #[input(stamp = generated_sources_stamp)]
    generated_sources: Option<CargoGeneratedSources>,
}

#[ajika::input(key = ())]
pub struct WorkspaceInput {
    #[input(arc)]
    target_cfg: CfgOptions,
}

#[ajika::input(key = CrateRef)]
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
