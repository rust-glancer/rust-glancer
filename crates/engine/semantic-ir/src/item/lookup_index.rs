//! Build header and written-name candidate hints for one semantic crate.
//!
//! Candidates belong to their own ItemStore; visible dependencies keep their own indexes.
//! Construction writes candidates directly into shards, then derives the keys used to select
//! those shards. Production publishes keys and shards independently and drops the complete index.
//! In-memory callers use the same key selection against their owned shards.
//!
//! For `impl Bag { fn len(&self) { ... } }`, the name index records which impl may provide `len`.
//! It does not need to construct that function's signature. An impl containing `methods!()` also
//! enters the macro fallback lists, since an unseen expansion may provide any requested name.

use std::{
    array,
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
};

use rg_ir_model::{
    AssocItemId, ImplRef, ItemStoreId, ItemStoreRef, Mutability, PrimitiveTy, TraitImplRef,
    TypeDefRef,
};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use super::{
    AssociatedItemHints,
    lookup_shard::{IndexedImplRef, IndexedTraitImplRef},
};
use crate::{AssociatedItemStore, ItemLookupShard, ItemStore, LookupKeys, LookupQuestion};

// Broad candidate lists use shard zero. Keyed tables share the hash used by lookup questions.
const SHARD_COUNT: usize = 16;

/// Outer receiver shape used to narrow saved trait impl declarations before semantic proof.
///
/// The value comes directly from the impl's source-shaped `Self` type. For example:
///
/// ```text
/// impl Marker for ()             -> Unit
/// impl Marker for u32            -> Primitive(UnsignedInt(U32))
/// impl Marker for (u8, bool)     -> Tuple(2)
/// impl<T> Marker for [T; 4]      -> Array
/// impl<T> Marker for [T]         -> Slice
/// impl Marker for &mut u8        -> Reference(Mutable)
/// impl Marker for *const u8      -> RawPointer(Shared)
/// impl Marker for fn(u8, bool)   -> FnPointer(2)
/// impl<T> Marker for Vec<T>      -> Adt(Vec)
/// ```
///
/// This is a candidate-routing key, not a shortened type. A tuple head keeps its arity but not its
/// field types, and an ADT head keeps the definition but not its generic arguments. The type layer
/// still checks the complete impl header before accepting a candidate.
///
/// This vocabulary is deliberately smaller than `rg_ty`'s canonical type vocabulary. Semantic IR
/// can identify direct structural syntax and a resolved nominal definition without normalizing
/// aliases or carrying body-owned identities. `impl<T> Marker for T`, an alias-headed impl, or an
/// incomplete header therefore has no head and enters the conservative fallback lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum TraitImplSelfHead {
    Unit,
    Never,
    Primitive(PrimitiveTy),
    /// Tuple shape and field count, such as `(u8, bool) -> Tuple(2)`.
    Tuple(u32),
    Array,
    Slice,
    Reference(Mutability),
    RawPointer(Mutability),
    /// Function-pointer shape and parameter count, such as `fn(u8) -> FnPointer(1)`.
    FnPointer(u32),
    /// Resolved nominal definition without its generic arguments, such as `Vec<u8> -> Adt(Vec)`.
    Adt(TypeDefRef),
}

/// The complete construction result for one target. Its fields stay private so keys always
/// describe the owned shards. Only the individual shards and keys are persisted.
#[derive(Debug, Clone, PartialEq, Eq, Default, MemorySize, Shrink)]
pub struct ItemLookupIndex {
    keys: LookupKeys,
    shards: [ItemLookupShard; SHARD_COUNT],
}

impl ItemLookupIndex {
    /// Build declaration-local candidates after semantic lowering has resolved impl headers.
    /// Synthetic stores without a definition resolver can pass an empty head map; their trait
    /// impls then enter the conservative fallback lane.
    pub fn build_from_store(
        store: &ItemStore,
        self_heads: &HashMap<ImplRef, TraitImplSelfHead>,
        members: &HashMap<ItemStoreId, AssociatedItemStore>,
    ) -> Self {
        Self::build(store, self_heads, |owner| {
            let mut hints = AssociatedItemHints::default();
            if let Some(members) = members.get(&owner.id) {
                for item in members.items() {
                    match item {
                        AssocItemId::Function(id) => hints
                            .functions
                            .extend(members.function_data(*id).map(|data| data.name.clone())),
                        AssocItemId::Const(id) => hints
                            .consts
                            .extend(members.const_data(*id).map(|data| data.name.clone())),
                        AssocItemId::TypeAlias(_) => hints.has_types = true,
                    }
                }
            }
            hints
        })
    }

    /// Saved targets supply hints from immediate member syntax. Building this index must not
    /// acquire the member stores it will later help select.
    pub(crate) fn build_from_headers(
        store: &ItemStore,
        self_heads: &HashMap<ImplRef, TraitImplSelfHead>,
        mut hints: HashMap<ItemStoreId, AssociatedItemHints>,
    ) -> Self {
        Self::build(store, self_heads, |owner| {
            hints.remove(&owner.id).unwrap_or_default()
        })
    }

    fn build(
        store: &ItemStore,
        self_heads: &HashMap<ImplRef, TraitImplSelfHead>,
        mut hints_for: impl FnMut(ItemStoreRef) -> AssociatedItemHints,
    ) -> Self {
        let mut shards: [ItemLookupShard; SHARD_COUNT] =
            array::from_fn(|_| ItemLookupShard::default());

        // Record trait declaration surfaces before processing impls. For `value.convert()`, the
        // name `convert` should select `Convert` before its impl headers are compared with value's
        // type. `value./* completion */` instead starts from every trait containing a function.
        for (trait_ref, _) in store.traits_with_refs() {
            let hints = hints_for(trait_ref.member_store());
            if hints.has_macros
                || hints.has_types
                || !hints.consts.is_empty()
                || !hints.functions.is_empty()
            {
                shards[0].traits_with_associated_items.push(trait_ref);
            }
            if hints.has_macros || !hints.functions.is_empty() {
                shards[0].traits_with_functions.push(trait_ref);
            }
            if hints.has_macros {
                shards[0].traits_with_macros.push(trait_ref);
            }
            let trait_shard = Self::shard_for(&trait_ref);
            shards[trait_shard].declared_traits.push(trait_ref);
            shards[trait_shard]
                .trait_impls_by_trait
                .entry(trait_ref)
                .or_default();
            for name in hints.functions {
                shards[Self::shard_for(&name)]
                    .traits_by_item_name
                    .entry(name)
                    .or_default()
                    .functions
                    .push(trait_ref);
            }
            for name in hints.consts {
                shards[Self::shard_for(&name)]
                    .traits_by_item_name
                    .entry(name)
                    .or_default()
                    .consts
                    .push(trait_ref);
            }
        }

        // Item-store lowering has already resolved impl headers into an expected-unique `Self`
        // type. Ambiguous nominal headers are not receiver-indexed. Structural inherent impls
        // need a small side list. Trait impls use the conservative source-level head computed by
        // the header pass, so receiver lookup does not need to reopen every canonical header.
        for (impl_ref, impl_data) in store.impls_with_refs() {
            if impl_data.trait_ref.is_none() {
                let self_ty = impl_data.resolved_self_ty.as_option().copied();
                if let Some(self_ty) = self_ty {
                    let shard = &mut shards[Self::shard_for(&self_ty)];
                    shard
                        .inherent_impls_by_type
                        .entry(self_ty)
                        .or_default()
                        .push(impl_ref);
                    shard
                        .inherent_function_owners_by_type_and_name
                        .entry(self_ty)
                        .or_default();
                } else if impl_data.resolved_self_ty.is_empty() {
                    // Inherent impls for shaped builtin types, such as `impl<T> [T]`, do not have
                    // a nominal receiver key. Keep them in a small side list so structural method
                    // lookup does not scan every visible impl.
                    shards[0].structural_inherent_impls.push(impl_ref);
                } else {
                    continue;
                }

                // Written names select owners, not member IDs. Macro-bearing owners stay in
                // a separate fallback lane because their output names do not exist yet.
                let hints = hints_for(impl_ref.member_store());
                if hints.has_macros {
                    if let Some(self_ty) = self_ty {
                        shards[Self::shard_for(&self_ty)]
                            .inherent_macro_impls_by_type
                            .entry(self_ty)
                            .or_default()
                            .push(impl_ref);
                    } else {
                        shards[0].structural_macro_impls.push(impl_ref);
                    }
                }
                for name in hints.functions {
                    let owners = if let Some(self_ty) = self_ty {
                        shards[Self::shard_for(&self_ty)]
                            .inherent_function_owners_by_type_and_name
                            .entry(self_ty)
                            .or_default()
                    } else {
                        &mut shards[Self::shard_for(&name)].structural_function_owners_by_name
                    };
                    owners.entry(name).or_default().push(impl_ref);
                }
            } else {
                let Some(trait_ref) = impl_data.resolved_trait_ref.as_option() else {
                    continue;
                };
                let trait_impl = TraitImplRef {
                    impl_ref,
                    trait_ref: *trait_ref,
                };

                // A nominal trait lookup reads both the trait shard for blanket fallbacks and
                // the type shard for direct candidates. These entries can live in different shards.
                shards[Self::shard_for(trait_ref)]
                    .trait_impls_by_trait
                    .entry(*trait_ref)
                    .or_default()
                    .push(
                        IndexedImplRef::from_crate(impl_ref),
                        self_heads.get(&impl_ref).copied(),
                    );
                if let Some(self_ty) = impl_data.resolved_self_ty.as_option() {
                    shards[Self::shard_for(self_ty)]
                        .trait_impls_by_type
                        .entry(*self_ty)
                        .or_default()
                        .push(IndexedTraitImplRef::from_crate(trait_impl));
                }
            }
        }

        // Derive routing keys from completed tables instead of maintaining a second set of
        // insertions. Missing keys must prove absence without overlooking a populated shard.
        let keys = LookupKeys::build(&shards);
        Self { keys, shards }
    }

    /// Publish keys and shard payloads independently, without persisting this aggregate.
    pub(crate) fn into_parts(self) -> (LookupKeys, [ItemLookupShard; SHARD_COUNT]) {
        (self.keys, self.shards)
    }

    /// In-memory readers select exactly the same shards as the persisted lookup provider.
    pub fn shards_for(&self, question: &LookupQuestion) -> impl Iterator<Item = &ItemLookupShard> {
        question
            .shards(&self.keys)
            .into_iter()
            .map(|shard| &self.shards[shard])
    }

    /// Hashing only routes reads: exact keys remain in each table. Changing this layout requires
    /// a cache epoch change, just like changing any other persisted output.
    pub(crate) fn shard_for(key: &impl Hash) -> usize {
        let mut hash = DefaultHasher::new();
        key.hash(&mut hash);
        (hash.finish() as usize) & (SHARD_COUNT - 1)
    }
}
