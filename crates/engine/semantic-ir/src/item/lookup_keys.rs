//! Keys that route lookup questions without loading candidate payloads.

use std::collections::HashSet;

use rg_ir_model::{TraitDefRef, TypeDefRef};
use rg_std::{MemorySize, Shrink};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use crate::ItemLookupShard;

/// A retained key means that its shard may answer a question. Absence is also meaningful:
/// shared preparation observes these keys before omitting that target's payload.
/// Written names alone cannot rule out a member macro. The macro flags keep the broad fallback
/// shard reachable even when the requested name has never appeared in the written-name table.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct LookupKeys {
    pub(crate) types: HashSet<TypeDefRef>,
    pub(crate) traits: HashSet<TraitDefRef>,
    pub(crate) names: HashSet<Name>,
    pub(crate) structural_impls: bool,
    pub(crate) structural_macros: bool,
    pub(crate) trait_macros: bool,
    pub(crate) traits_with_functions: bool,
    pub(crate) traits_with_associated_items: bool,
}

impl LookupKeys {
    pub(crate) fn build(shards: &[ItemLookupShard]) -> Self {
        let mut keys = Self {
            types: shards
                .iter()
                .flat_map(|shard| {
                    shard
                        .inherent_impls_by_type
                        .keys()
                        .chain(shard.inherent_function_owners_by_type_and_name.keys())
                        .chain(shard.trait_impls_by_type.keys())
                        .copied()
                })
                .collect(),
            traits: shards
                .iter()
                .flat_map(|shard| {
                    shard
                        .trait_impls_by_trait
                        .keys()
                        .chain(shard.declared_traits.iter())
                        .copied()
                })
                .collect(),
            names: shards
                .iter()
                .flat_map(|shard| {
                    shard
                        .structural_function_owners_by_name
                        .keys()
                        .chain(shard.traits_by_item_name.keys())
                        .cloned()
                })
                .collect(),
            structural_macros: shards
                .iter()
                .any(|shard| !shard.structural_macro_impls.is_empty()),
            trait_macros: shards
                .iter()
                .any(|shard| !shard.traits_with_macros.is_empty()),
            structural_impls: shards
                .iter()
                .any(|shard| !shard.structural_inherent_impls.is_empty()),
            traits_with_functions: shards
                .iter()
                .any(|shard| !shard.traits_with_functions.is_empty()),
            traits_with_associated_items: shards
                .iter()
                .any(|shard| !shard.traits_with_associated_items.is_empty()),
        };
        keys.shrink_to_fit();
        keys
    }
}
