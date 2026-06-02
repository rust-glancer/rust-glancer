//! Precomputed lookup indexes over semantic-shaped item stores.
//!
//! Method and deref queries ask the same receiver-based questions many times. This index pays the
//! visible-store scan once and lets later query code jump straight to plausible impl/function
//! candidates while preserving the normal item-store query semantics for the final checks.

use std::collections::HashMap;

use rg_ir_model::{AssocItemId, FunctionRef, ImplRef, TraitImplRef, TraitRef, TypeDefRef};
use rg_text::Name;

use crate::{ItemStoreQuery, ItemStoreSource, push_unique};

/// Receiver-oriented lookup cache built from the stores visible to an `ItemStoreQuery`.
#[derive(Debug, Default)]
pub struct ItemLookupIndex {
    // Method lookup starts from a receiver type. These maps let callers jump directly to impls
    // whose already-resolved `Self` type mentions that receiver, instead of re-scanning all impls.
    inherent_impls_by_type: HashMap<TypeDefRef, Vec<ImplRef>>,
    inherent_functions_by_type_and_name: HashMap<TypeDefRef, HashMap<Name, Vec<FunctionRef>>>,
    trait_impls_by_type: HashMap<TypeDefRef, Vec<TraitImplRef>>,
    // Trait impl lookup produces trait identities first; this cache then expands each trait into
    // its associated function declarations without reopening the trait item every time.
    trait_functions_by_trait: HashMap<TraitRef, Vec<FunctionRef>>,
    trait_functions_by_trait_and_name: HashMap<TraitRef, HashMap<Name, Vec<FunctionRef>>>,
}

impl ItemLookupIndex {
    /// Builds an index from the stores that are visible to broad item lookup.
    pub fn build<'item, S>(item_query: &ItemStoreQuery<'item, S>) -> Result<Self, S::Error>
    where
        S: ItemStoreSource<'item>,
    {
        let mut index = Self::default();

        // The index mirrors broad item-store lookup helpers, but pays the store scan once up front
        // instead of once per method expression.
        for store in item_query.visible_stores()? {
            // Trait methods are independent of a receiver type, so cache them by trait before
            // processing impls that later point back to these traits.
            for (trait_ref, trait_data) in store.traits_with_refs() {
                let functions = index.trait_functions_by_trait.entry(trait_ref).or_default();
                index
                    .trait_functions_by_trait_and_name
                    .entry(trait_ref)
                    .or_default();
                for item in &trait_data.items {
                    if let AssocItemId::Function(id) = item {
                        let function_ref = FunctionRef {
                            origin: trait_ref.origin,
                            id: *id,
                        };
                        push_unique(functions, function_ref);
                        if let Some(function_data) = item_query.function_data(function_ref)? {
                            push_unique(
                                index
                                    .trait_functions_by_trait_and_name
                                    .entry(trait_ref)
                                    .or_default()
                                    .entry(function_data.name.clone())
                                    .or_default(),
                                function_ref,
                            );
                        }
                    }
                }
            }

            // Item-store lowering has already resolved impl headers into possible `Self` types.
            // The index preserves that optimistic shape: ambiguous impls are attached to every
            // resolved self type, and later applicability checks still decide whether candidates fit.
            for (impl_ref, impl_data) in store.impls_with_refs() {
                if impl_data.trait_ref.is_none() {
                    for self_ty in &impl_data.resolved_self_tys {
                        push_unique(
                            index.inherent_impls_by_type.entry(*self_ty).or_default(),
                            impl_ref,
                        );
                        for item in &impl_data.items {
                            if let AssocItemId::Function(id) = item {
                                let function_ref = FunctionRef {
                                    origin: impl_ref.origin,
                                    id: *id,
                                };
                                let Some(function_data) = item_query.function_data(function_ref)?
                                else {
                                    continue;
                                };
                                push_unique(
                                    index
                                        .inherent_functions_by_type_and_name
                                        .entry(*self_ty)
                                        .or_default()
                                        .entry(function_data.name.clone())
                                        .or_default(),
                                    function_ref,
                                );
                            }
                        }
                    }
                } else {
                    for self_ty in &impl_data.resolved_self_tys {
                        let trait_impls = index.trait_impls_by_type.entry(*self_ty).or_default();
                        for trait_ref in &impl_data.resolved_trait_refs {
                            push_unique(
                                trait_impls,
                                TraitImplRef {
                                    impl_ref,
                                    trait_ref: *trait_ref,
                                },
                            );
                        }
                    }
                }
            }
        }

        Ok(index)
    }

    /// Expands indexed inherent impls to their function items through the caller's query source.
    pub fn inherent_functions_for_type<'item, S>(
        &self,
        item_query: &ItemStoreQuery<'item, S>,
        ty: TypeDefRef,
    ) -> Result<Vec<FunctionRef>, S::Error>
    where
        S: ItemStoreSource<'item>,
    {
        let mut functions = Vec::new();
        let Some(impl_refs) = self.inherent_impls_by_type.get(&ty) else {
            return Ok(functions);
        };

        // Store impl ids, not function ids, because function lists belong to impl item data. This
        // keeps the index compact while still avoiding the expensive global impl search.
        for impl_ref in impl_refs {
            let Some(data) = item_query.impl_data(*impl_ref)? else {
                continue;
            };

            for item in &data.items {
                if let AssocItemId::Function(id) = item {
                    push_unique(
                        &mut functions,
                        FunctionRef {
                            origin: impl_ref.origin,
                            id: *id,
                        },
                    );
                }
            }
        }

        Ok(functions)
    }

    /// Returns same-name inherent functions indexed for a receiver type.
    pub fn inherent_functions_for_type_and_name(
        &self,
        ty: TypeDefRef,
        name: &str,
    ) -> &[FunctionRef] {
        // Dot lookup almost always starts with the method name already known. Keeping the name as
        // part of the key lets callers avoid checking receiver applicability for unrelated methods.
        self.inherent_functions_by_type_and_name
            .get(&ty)
            .and_then(|functions_by_name| functions_by_name.get(name))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Returns trait impl candidates indexed for a receiver type.
    pub fn trait_impls_for_type(&self, ty: TypeDefRef) -> &[TraitImplRef] {
        self.trait_impls_by_type
            .get(&ty)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Returns trait-declared functions if the trait was visible when the index was built.
    pub fn trait_functions(&self, trait_ref: TraitRef) -> Option<&[FunctionRef]> {
        // `None` means the trait was not visible while this index was built. Callers can then fall
        // back to the direct item-store query for cross-subset/offloaded edge cases.
        self.trait_functions_by_trait
            .get(&trait_ref)
            .map(Vec::as_slice)
    }

    /// Returns same-name trait functions if the trait was visible when the index was built.
    pub fn trait_functions_by_name(
        &self,
        trait_ref: TraitRef,
        name: &str,
    ) -> Option<&[FunctionRef]> {
        // `Some(&[])` is meaningful: the trait is indexed and has no function with this name, so
        // callers can skip the trait-impl applicability check entirely for this method lookup.
        let functions_by_name = self.trait_functions_by_trait_and_name.get(&trait_ref)?;
        Some(
            functions_by_name
                .get(name)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        )
    }
}
