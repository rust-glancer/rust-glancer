//! Profile descriptors for body inference and resolution.

use rg_profile::{ProfileDescriptor, declare_metrics};

declare_metrics! {
    pub(crate) mod metric {
        scope "body_ir.resolution" {
            /// Bodies that retained semantic progress until the fixed-point safety limit.
            counter FIXED_POINT_EXHAUSTIONS = "fixed_point_exhaustions";
        }
    }
}

pub fn profile_descriptors() -> &'static [ProfileDescriptor] {
    metric::descriptors()
}
