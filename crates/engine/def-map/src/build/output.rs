use crate::{CrateData, GeneratedItemStore, MacroExpansionLimitReport};

/// Completed data from one crate. Publishing this result never needs the mutable builder.
pub struct CrateBuildOutput {
    pub data: CrateData,
    pub generated_items: GeneratedItemStore,
    pub macro_expansion_limit: Option<MacroExpansionLimitReport>,
}
