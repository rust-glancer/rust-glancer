use rg_cfg_eval::CfgOptions;
use rg_ir_model::PackageSlot;
use rg_text::RustEdition;

#[ajika::input(key = PackageSlot)]
pub struct PackageInput {
    edition: RustEdition,
    #[input(arc)]
    cfg: CfgOptions,
}

#[ajika::input(key = ())]
pub struct WorkspaceInput {
    #[input(arc)]
    target_cfg: CfgOptions,
}
