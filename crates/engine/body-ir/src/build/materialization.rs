//! Selects saved targets for body analysis or transient declaration initializer evaluation.

use rg_ir_model::CrateRef;
use rg_std::UniqueVec;

use crate::{BodyIrBuildPolicy, CrateBodiesCoverage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BodyIrMaterializationPlan {
    ConfiguredBodies(BodyIrBuildPolicy),
    SelectedCrates(UniqueVec<CrateRef>),
}

impl BodyIrMaterializationPlan {
    pub(super) fn lowering(&self) -> BodyIrMaterialization<'_> {
        match self {
            Self::ConfiguredBodies(policy) => BodyIrMaterialization::ConfiguredBodies(*policy),
            Self::SelectedCrates(crates) => BodyIrMaterialization::SelectedCrates(crates),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum BodyIrMaterialization<'a> {
    DeclarationInitializers,
    ConfiguredBodies(BodyIrBuildPolicy),
    SelectedCrates(&'a UniqueVec<CrateRef>),
}

impl BodyIrMaterialization<'_> {
    pub(super) fn crate_coverage(
        self,
        parse_package: &rg_parse::Package,
        parse_target: &rg_parse::CargoTarget,
    ) -> CrateBodiesCoverage {
        match self {
            Self::ConfiguredBodies(policy)
                if !policy.should_lower_target(parse_package, parse_target) =>
            {
                CrateBodiesCoverage::SkippedByPolicy
            }
            // Initializer coverage is transient. Selected targets are always completed in full.
            _ => CrateBodiesCoverage::Complete,
        }
    }

    pub(super) fn selects_crate(self, crate_ref: CrateRef) -> bool {
        match self {
            Self::SelectedCrates(crates) => crates.contains(&crate_ref),
            Self::ConfiguredBodies(_) | Self::DeclarationInitializers => true,
        }
    }
}
