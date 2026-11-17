//! Keys that route lookup questions without loading candidate payloads.

use std::collections::HashSet;

use rg_ir_model::{TraitDefRef, TypeDefRef};
use rg_std::{MemorySize, Shrink};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use crate::ItemLookupShard;

/// A retained key means that its shard may answer a question. Absence is also meaningful:
/// shared preparation observes these keys before omitting that target's payload.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct LookupKeys {
    pub(crate) types: HashSet<TypeDefRef>,
    pub(crate) traits: HashSet<TraitDefRef>,
    pub(crate) names: HashSet<Name>,
    pub(crate) structural_impls: bool,
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
                        .chain(shard.inherent_functions_by_type_and_name.keys())
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
                        .chain(shard.trait_functions_by_trait.keys())
                        .chain(shard.trait_functions_by_trait_and_name.keys())
                        .copied()
                })
                .collect(),
            names: shards
                .iter()
                .flat_map(|shard| {
                    shard
                        .structural_inherent_functions_by_name
                        .keys()
                        .chain(shard.traits_by_item_name.keys())
                        .cloned()
                })
                .collect(),
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
