//! One independently persisted shard of a target's lookup candidates.

use std::collections::HashMap;

use rg_ir_model::{
    CrateRef, DefMapRef, FunctionRef, ImplId, ImplRef, TraitDefRef, TraitId, TraitImplRef,
    TypeDefRef,
};
use rg_std::{MemorySize, Shrink, UniqueVec};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use crate::TraitImplSelfHead;

/// Candidate tables selected by a receiver, trait, or name key. A shard contains only part of
/// one target's candidates; callers acquire every shard selected by their lookup question.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ItemLookupShard {
    // Inherent lookup starts from a receiver type. These maps jump directly to impls whose
    // already-resolved `Self` type mentions that receiver.
    pub(crate) inherent_impls_by_type: HashMap<TypeDefRef, UniqueVec<ImplRef>>,
    pub(crate) inherent_functions_by_type_and_name:
        HashMap<TypeDefRef, HashMap<Name, UniqueVec<FunctionRef>>>,
    pub(crate) structural_inherent_impls: UniqueVec<ImplRef>,
    pub(crate) structural_inherent_functions_by_name: HashMap<Name, UniqueVec<FunctionRef>>,
    // Implementation navigation and qualified paths still ask which impls mention one nominal
    // type. Trait-item lookup uses the trait-keyed map below instead.
    pub(crate) trait_impls_by_type: HashMap<TypeDefRef, UniqueVec<IndexedTraitImplRef>>,
    pub(crate) trait_impls_by_trait: HashMap<TraitDefRef, IndexedTraitImpls>,
    // A named trait item selects its declaring traits before any impl proof. Completion starts
    // from one of the two broad trait surfaces. Once a trait is selected, the function maps adapt
    // its proof back into declarations without reopening the trait item every time.
    pub(crate) traits_with_functions: UniqueVec<TraitDefRef>,
    pub(crate) traits_with_associated_items: UniqueVec<TraitDefRef>,
    pub(crate) traits_by_item_name: HashMap<Name, TraitItemTraitRefs>,
    pub(crate) trait_functions_by_trait: HashMap<TraitDefRef, UniqueVec<FunctionRef>>,
    pub(crate) trait_functions_by_trait_and_name:
        HashMap<TraitDefRef, HashMap<Name, UniqueVec<FunctionRef>>>,
}

impl ItemLookupShard {
    /// Count retained candidate references across every declaration-local lookup table.
    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> usize {
        self.inherent_impls_by_type
            .values()
            .map(UniqueVec::len)
            .sum::<usize>()
            + self
                .inherent_functions_by_type_and_name
                .values()
                .flat_map(HashMap::values)
                .map(UniqueVec::len)
                .sum::<usize>()
            + self.structural_inherent_impls.len()
            + self
                .structural_inherent_functions_by_name
                .values()
                .map(UniqueVec::len)
                .sum::<usize>()
            + self
                .trait_impls_by_type
                .values()
                .map(UniqueVec::len)
                .sum::<usize>()
            + self
                .trait_impls_by_trait
                .values()
                .map(IndexedTraitImpls::entry_count)
                .sum::<usize>()
            + self.traits_with_functions.len()
            + self.traits_with_associated_items.len()
            + self
                .traits_by_item_name
                .values()
                .map(TraitItemTraitRefs::entry_count)
                .sum::<usize>()
            + self
                .trait_functions_by_trait
                .values()
                .map(UniqueVec::len)
                .sum::<usize>()
            + self
                .trait_functions_by_trait_and_name
                .values()
                .flat_map(HashMap::values)
                .map(UniqueVec::len)
                .sum::<usize>()
    }

    /// Return one trait's structural receiver-head candidates followed by conservative fallbacks.
    ///
    /// Given these declarations:
    ///
    /// ```text
    /// impl Marker for u32 {}
    /// impl<T> Marker for [T] {}
    /// impl<T> Marker for T {}
    /// ```
    ///
    /// a `Primitive(UnsignedInt(U32))` lookup returns the first and third impls. A `Slice` lookup
    /// returns the second and third. The exact matcher later checks everything omitted by the head
    /// key.
    ///
    /// Nominal receiver candidates live in type-keyed shards and are merged by the caller.
    /// `None` asks only for fallbacks. Closure and function-item receivers use that lane because a
    /// concrete source impl cannot name their body-owned identity.
    pub(crate) fn trait_impl_candidates_for_self_head(
        &self,
        trait_ref: TraitDefRef,
        self_head: Option<TraitImplSelfHead>,
    ) -> Option<UniqueVec<IndexedImplRef>> {
        let indexed = self.trait_impls_by_trait.get(&trait_ref)?;
        let mut impls = UniqueVec::new();

        if let Some(self_head) = self_head
            && let Some(candidates) = indexed.direct_by_self_head.get(&self_head)
        {
            impls.extend(candidates.iter().copied());
        }
        impls.extend(indexed.fallbacks.iter().copied());
        Some(impls)
    }
}

/// All impls of one trait plus the receiver lanes used by impl discovery.
///
/// For `impl Marker for u32`, `impl<T> Marker for [T]`, and `impl<T> Marker for T`, `all` contains
/// all three declarations, `direct_by_self_head` has primitive and slice entries, and `fallbacks`
/// contains only the blanket impl. This lets a known `u32` receiver avoid reopening the slice impl
/// while still considering the blanket one.
///
/// Nominal direct impls already live in `trait_impls_by_type`, so this value duplicates only
/// structural and fallback impl identities. The complete list remains necessary for trait solver enumeration,
/// implementation navigation, and unresolved receiver fallbacks.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub(crate) struct IndexedTraitImpls {
    pub(crate) all: UniqueVec<IndexedImplRef>,
    fallbacks: UniqueVec<IndexedImplRef>,
    direct_by_self_head: HashMap<TraitImplSelfHead, UniqueVec<IndexedImplRef>>,
}

impl IndexedTraitImpls {
    pub(crate) fn push(&mut self, impl_ref: IndexedImplRef, self_head: Option<TraitImplSelfHead>) {
        if !self.all.push(impl_ref) {
            return;
        }

        match self_head {
            Some(TraitImplSelfHead::Adt(_)) => {
                // The exact definition key and implemented trait are already retained by
                // `trait_impls_by_type`; avoid storing a third copy here.
            }
            Some(self_head) => {
                self.direct_by_self_head
                    .entry(self_head)
                    .or_default()
                    .push(impl_ref);
            }
            None => {
                self.fallbacks.push(impl_ref);
            }
        }
    }

    #[cfg(test)]
    fn entry_count(&self) -> usize {
        self.all.len()
            + self.fallbacks.len()
            + self
                .direct_by_self_head
                .values()
                .map(UniqueVec::len)
                .sum::<usize>()
    }
}

/// Declaring traits partitioned by the kind of one shared associated-item name.
///
/// Rust allows different traits to use the same spelling for different associated-item kinds.
/// Keeping the lanes together gives name-first lookup one persisted map without making a method
/// query consider an unrelated associated const.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub(crate) struct TraitItemTraitRefs {
    pub(crate) functions: UniqueVec<TraitDefRef>,
    pub(crate) consts: UniqueVec<TraitDefRef>,
}

impl TraitItemTraitRefs {
    #[cfg(test)]
    fn entry_count(&self) -> usize {
        self.functions.len() + self.consts.len()
    }
}

/// Crate-only impl identity retained inside a crate-local lookup index.
///
/// A general [`ImplRef`] also has to represent body-local declarations, which makes its
/// [`rg_ir_model::DefMapRef`] origin larger. Entries collected from semantic item stores cannot be
/// body-local, so retaining the crate directly avoids paying for that unused variant millions of
/// times. Query methods expand this back to the ordinary public identity at their boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub(crate) struct IndexedImplRef {
    crate_ref: CrateRef,
    id: ImplId,
}

impl IndexedImplRef {
    pub(crate) fn from_crate(impl_ref: ImplRef) -> Self {
        Self {
            crate_ref: impl_ref
                .origin
                .as_crate_ref()
                .expect("semantic item-store impl should have a crate origin"),
            id: impl_ref.id,
        }
    }

    pub(crate) fn expand(self) -> ImplRef {
        ImplRef::new(DefMapRef::Crate(self.crate_ref), self.id)
    }
}

/// Compact trait impl identity used where the trait is not already present as the map key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub(crate) struct IndexedTraitImplRef {
    impl_ref: IndexedImplRef,
    trait_crate: CrateRef,
    trait_id: TraitId,
}

impl IndexedTraitImplRef {
    pub(crate) fn from_crate(trait_impl: TraitImplRef) -> Self {
        Self {
            impl_ref: IndexedImplRef::from_crate(trait_impl.impl_ref),
            trait_crate: trait_impl
                .trait_ref
                .origin
                .as_crate_ref()
                .expect("semantic item-store trait should have a crate origin"),
            trait_id: trait_impl.trait_ref.id,
        }
    }

    pub(crate) fn expand(self) -> TraitImplRef {
        TraitImplRef {
            impl_ref: self.impl_ref.expand(),
            trait_ref: TraitDefRef::new(DefMapRef::Crate(self.trait_crate), self.trait_id),
        }
    }
}
