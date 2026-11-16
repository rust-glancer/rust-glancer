use std::collections::HashMap;

use super::macros::{AssociatedItemInput, AssociatedItemOwner, MacroScopeData};
use crate::{CrateData, GeneratedItemStore, MacroExpansionLimitReport};

/// Completed data from one crate. Publishing this result never needs the mutable builder.
///
/// Module collection has finished, but trait/impl member macros are still deferred. Their inputs
/// and macro scopes travel with this result so later queries can expand them after the builder
/// and its work queues have been dropped.
pub struct CrateBuildOutput {
    pub data: CrateData,
    pub member_inputs: HashMap<AssociatedItemOwner, AssociatedItemInput>,
    pub macro_scopes: MacroScopeData,
    pub generated_items: GeneratedItemStore,
    pub macro_expansion_limit: Option<MacroExpansionLimitReport>,
}
