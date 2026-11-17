//! Select declaration candidates from acquired lookup shards. These answers do not prove types.

use rg_ir_model::{
    AssocItemId, FunctionRef, ImplRef, ItemStoreRef, TraitDefRef, TraitImplRef, TypeDefRef,
};
use rg_source::ReadError;
use rg_std::UniqueVec;
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use crate::{AssociatedItemStore, ItemLookupIndex, ItemLookupShard, LookupKeys, TraitImplSelfHead};

/// A candidate question contains identities and names, never an inference table or a source span.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite)]
pub enum LookupQuestion {
    InherentFunctions(TypeDefRef, Name),
    StructuralFunctions(Name),
    StructuralImpls,
    InherentImpls(TypeDefRef),
    ImplsForType(TypeDefRef),
    ImplsForTrait(TraitDefRef),
    TraitImplsForType(TypeDefRef),
    TraitImpls(TraitDefRef),
    TraitImplsForHead(TraitDefRef, Option<TraitImplSelfHead>),
    TraitsWithFunctions,
    TraitsWithAssociatedItems,
    TraitsWithFunctionName(Name),
    TraitsWithConstName(Name),
    TraitFunctions(TraitDefRef),
    TraitFunctionsByName(TraitDefRef, Name),
}

impl LookupQuestion {
    pub(crate) fn shards(&self, keys: &LookupKeys) -> UniqueVec<usize> {
        let (present, shard) = match self {
            Self::InherentFunctions(ty, _)
            | Self::InherentImpls(ty)
            | Self::ImplsForType(ty)
            | Self::TraitImplsForType(ty) => {
                (keys.types.contains(ty), ItemLookupIndex::shard_for(ty))
            }
            Self::ImplsForTrait(trait_ref)
            | Self::TraitImpls(trait_ref)
            | Self::TraitImplsForHead(trait_ref, _)
            | Self::TraitFunctions(trait_ref)
            | Self::TraitFunctionsByName(trait_ref, _) => (
                keys.traits.contains(trait_ref),
                ItemLookupIndex::shard_for(trait_ref),
            ),
            Self::StructuralFunctions(name)
            | Self::TraitsWithFunctionName(name)
            | Self::TraitsWithConstName(name) => {
                (keys.names.contains(name), ItemLookupIndex::shard_for(name))
            }
            Self::StructuralImpls => (keys.structural_impls, 0),
            Self::TraitsWithFunctions => (keys.traits_with_functions, 0),
            Self::TraitsWithAssociatedItems => (keys.traits_with_associated_items, 0),
        };
        let mut shards = UniqueVec::new();
        if present {
            shards.push(shard);
        }
        // A missing written name does not rule out a macro-generated member. Include the broad
        // macro-owner shard too, so `methods!()` can supply a name absent from the written index.
        match self {
            Self::StructuralFunctions(_) if keys.structural_macros => {
                shards.push(0);
            }
            Self::TraitsWithFunctionName(_) | Self::TraitsWithConstName(_) if keys.trait_macros => {
                shards.push(0);
            }
            _ => {}
        }
        // Nominal receiver candidates are indexed by type; trait presence and blanket fallbacks
        // are indexed by trait. Read both when the keys include the nominal receiver.
        if let Self::TraitImplsForHead(_, Some(TraitImplSelfHead::Adt(ty))) = self
            && keys.types.contains(ty)
        {
            shards.push(ItemLookupIndex::shard_for(ty));
        }
        shards
    }

    /// Union selected shards in visibility order, then read members only for function answers.
    /// Impl and trait candidate lists come from headers and hints alone; they may still need
    /// semantic proof. The provider observes routing keys even when no shard matches, so adding
    /// the first matching declaration invalidates an earlier empty answer.
    pub fn answer<'a, 'item>(
        &self,
        shards: impl Iterator<Item = &'a ItemLookupShard>,
        mut members: impl FnMut(ItemStoreRef) -> Result<Option<&'item AssociatedItemStore>, ReadError>,
    ) -> Result<LookupAnswer, ReadError> {
        let mut functions = UniqueVec::new();
        let mut function_owners = UniqueVec::new();
        let mut impls = UniqueVec::new();
        let mut traits = UniqueVec::new();
        let mut trait_impls = UniqueVec::new();
        let mut fallback_impls = UniqueVec::new();
        let mut known = false;
        for shard in shards {
            match self {
                Self::InherentFunctions(ty, name) => {
                    function_owners.extend(
                        shard
                            .inherent_function_owners_by_type_and_name
                            .get(ty)
                            .and_then(|names| names.get(name))
                            .into_iter()
                            .flatten()
                            .chain(
                                shard
                                    .inherent_macro_impls_by_type
                                    .get(ty)
                                    .into_iter()
                                    .flatten(),
                            )
                            .map(|owner| owner.member_store()),
                    );
                }
                Self::StructuralFunctions(name) => {
                    function_owners.extend(
                        shard
                            .structural_function_owners_by_name
                            .get(name)
                            .into_iter()
                            .flatten()
                            .chain(shard.structural_macro_impls.iter())
                            .map(|owner| owner.member_store()),
                    );
                }
                Self::StructuralImpls => {
                    impls.extend(shard.structural_inherent_impls.iter().copied())
                }
                Self::InherentImpls(ty) => impls.extend(
                    shard
                        .inherent_impls_by_type
                        .get(ty)
                        .into_iter()
                        .flatten()
                        .copied(),
                ),
                Self::ImplsForType(ty) => {
                    impls.extend(
                        shard
                            .inherent_impls_by_type
                            .get(ty)
                            .into_iter()
                            .flatten()
                            .copied(),
                    );
                    impls.extend(
                        shard
                            .trait_impls_by_type
                            .get(ty)
                            .into_iter()
                            .flatten()
                            .map(|candidate| candidate.expand().impl_ref),
                    );
                }
                Self::ImplsForTrait(trait_ref) => impls.extend(
                    shard
                        .trait_impls_by_trait
                        .get(trait_ref)
                        .into_iter()
                        .flat_map(|candidates| {
                            candidates.all.iter().map(|candidate| candidate.expand())
                        }),
                ),
                Self::TraitImplsForType(ty) => trait_impls.extend(
                    shard
                        .trait_impls_by_type
                        .get(ty)
                        .into_iter()
                        .flatten()
                        .map(|candidate| candidate.expand()),
                ),
                Self::TraitImpls(trait_ref) => {
                    if let Some(candidates) = shard.trait_impls_by_trait.get(trait_ref) {
                        known = true;
                        trait_impls.extend(candidates.all.iter().map(|candidate| TraitImplRef {
                            impl_ref: candidate.expand(),
                            trait_ref: *trait_ref,
                        }));
                    }
                }
                Self::TraitImplsForHead(trait_ref, head) => {
                    let head = match head {
                        Some(TraitImplSelfHead::Adt(ty)) => {
                            trait_impls.extend(
                                shard
                                    .trait_impls_by_type
                                    .get(ty)
                                    .into_iter()
                                    .flatten()
                                    .map(|candidate| candidate.expand())
                                    .filter(|candidate| candidate.trait_ref == *trait_ref),
                            );
                            None
                        }
                        head => *head,
                    };
                    if let Some(candidates) =
                        shard.trait_impl_candidates_for_self_head(*trait_ref, head)
                    {
                        known = true;
                        fallback_impls.extend(candidates.into_iter().map(|candidate| {
                            TraitImplRef {
                                impl_ref: candidate.expand(),
                                trait_ref: *trait_ref,
                            }
                        }));
                    }
                }
                Self::TraitsWithFunctions => {
                    traits.extend(shard.traits_with_functions.iter().copied())
                }
                Self::TraitsWithAssociatedItems => {
                    traits.extend(shard.traits_with_associated_items.iter().copied())
                }
                Self::TraitsWithFunctionName(name) => traits.extend(
                    shard
                        .traits_by_item_name
                        .get(name)
                        .into_iter()
                        .flat_map(|refs| refs.functions.iter().copied())
                        .chain(shard.traits_with_macros.iter().copied()),
                ),
                Self::TraitsWithConstName(name) => traits.extend(
                    shard
                        .traits_by_item_name
                        .get(name)
                        .into_iter()
                        .flat_map(|refs| refs.consts.iter().copied())
                        .chain(shard.traits_with_macros.iter().copied()),
                ),
                Self::TraitFunctions(trait_ref) | Self::TraitFunctionsByName(trait_ref, _) => {
                    if shard.declared_traits.contains(trait_ref) {
                        known = true;
                        function_owners.push(trait_ref.member_store());
                    }
                }
            }
        }
        let name = match self {
            Self::InherentFunctions(_, name)
            | Self::StructuralFunctions(name)
            | Self::TraitFunctionsByName(_, name) => Some(name),
            _ => None,
        };
        // Hints selected owners, not function IDs. Open each selected store and check the real
        // names after macro expansion; only then can a function answer name its declaration.
        for owner in function_owners {
            let Some(store) = members(owner)? else {
                continue;
            };
            for item in store.items() {
                if let AssocItemId::Function(id) = item
                    && store
                        .function_data(*id)
                        .is_some_and(|data| name.is_none_or(|name| *name == data.name))
                {
                    functions.push(FunctionRef::new(owner, *id));
                }
            }
        }
        trait_impls.extend(fallback_impls);
        Ok(match self {
            Self::InherentFunctions(..) | Self::StructuralFunctions(..) => {
                LookupAnswer::Functions(Some(functions))
            }
            Self::TraitFunctions(..) | Self::TraitFunctionsByName(..) => {
                LookupAnswer::Functions(known.then_some(functions))
            }
            Self::StructuralImpls
            | Self::InherentImpls(..)
            | Self::ImplsForType(..)
            | Self::ImplsForTrait(..) => LookupAnswer::Impls(impls),
            Self::TraitImplsForType(..) => LookupAnswer::TraitImpls(Some(trait_impls)),
            Self::TraitImpls(..) | Self::TraitImplsForHead(..) => {
                LookupAnswer::TraitImpls(known.then_some(trait_impls))
            }
            _ => LookupAnswer::Traits(traits),
        })
    }
}

/// Trait-keyed answers retain the difference between an unknown trait and an empty candidate set.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub enum LookupAnswer {
    Functions(Option<UniqueVec<FunctionRef>>),
    Impls(UniqueVec<ImplRef>),
    Traits(UniqueVec<TraitDefRef>),
    TraitImpls(Option<UniqueVec<TraitImplRef>>),
}

impl LookupAnswer {
    pub(crate) fn extend(&mut self, other: Self) {
        match (self, other) {
            (Self::Functions(left), Self::Functions(right)) => {
                if let Some(right) = right {
                    left.get_or_insert_default().extend(right);
                }
            }
            (Self::TraitImpls(left), Self::TraitImpls(right)) => {
                if let Some(right) = right {
                    left.get_or_insert_default().extend(right);
                }
            }
            (Self::Impls(left), Self::Impls(right)) => left.extend(right),
            (Self::Traits(left), Self::Traits(right)) => left.extend(right),
            _ => unreachable!("candidate unions answer the same question"),
        }
    }
}
