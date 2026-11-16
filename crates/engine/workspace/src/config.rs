use std::sync::Arc;

use rg_cfg_eval::{CfgEvaluator, CfgOptions};
use rg_ir_model::CrateRef;
use rg_text::RustEdition;

use crate::{PackageInput, TargetInput};

/// The language settings for one crate interpretation of source.
///
/// A package's library and tests share an edition and Cargo features, but test targets also
/// enable `cfg(test)`. Saved lowering and editor-source lowering must apply that same policy.
/// This value borrows no syntax and says nothing about which files belong to the crate.
pub struct CrateConfig {
    pub edition: RustEdition,
    cfg: Arc<CfgOptions>,
    test_cfg: bool,
}

impl CrateConfig {
    pub fn read<S: ajika::Services>(db: &ajika::Db<'_, S>, krate: CrateRef) -> ajika::Result<Self> {
        let package = PackageInput::at(krate.package);
        Ok(Self {
            edition: package.edition(db)?,
            cfg: package.cfg(db)?,
            test_cfg: TargetInput::at(krate).kind(db)?.enables_test_cfg(),
        })
    }

    pub fn cfg(&self) -> CfgEvaluator<'_> {
        CfgEvaluator::new(&self.cfg, self.test_cfg)
    }
}
