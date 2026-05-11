//! Transient lookup indexes used while resolving bodies.
//!
//! Body resolution asks method-lookup questions for many expressions. Building this small index
//! once per resolution pass avoids repeatedly scanning every semantic impl in every package.

use std::collections::HashMap;

use rg_package_store::PackageStoreError;
use rg_semantic_ir::{
    AssocItemId, FunctionRef, ImplRef, SemanticIrReadTxn, TraitImplRef, TraitRef, TypeDefRef,
};
use rg_text::Name;

use super::push_unique;

#[derive(Debug, Default)]
pub(crate) struct SemanticResolutionIndex {
    // Method lookup starts from a receiver type. These maps let the resolver jump directly to impls
    // whose already-resolved `Self` type mentions that receiver, instead of re-scanning all impls.
    inherent_impls_by_type: HashMap<TypeDefRef, Vec<ImplRef>>,
    inherent_functions_by_type_and_name: HashMap<TypeDefRef, HashMap<Name, Vec<FunctionRef>>>,
    trait_impls_by_type: HashMap<TypeDefRef, Vec<TraitImplRef>>,
    // Trait impl lookup produces trait identities first; this cache then expands each trait into
    // its associated function declarations without reopening the trait item every time.
    trait_functions_by_trait: HashMap<TraitRef, Vec<FunctionRef>>,
    trait_functions_by_trait_and_name: HashMap<TraitRef, HashMap<Name, Vec<FunctionRef>>>,
}

impl SemanticResolutionIndex {
    pub(crate) fn build(semantic_ir: &SemanticIrReadTxn<'_>) -> Result<Self, PackageStoreError> {
        let mut index = Self::default();

        // The index mirrors Semantic IR's broad lookup helpers, but pays the package-wide scan
        // once up front instead of once per method expression.
        for (target, _) in semantic_ir.materialize_included_target_irs()? {
            // Trait methods are independent of a receiver type, so we can cache them by trait
            // before processing impls that will later point back to these traits.
            for (trait_ref, trait_data) in semantic_ir.traits(target)? {
                let functions = index.trait_functions_by_trait.entry(trait_ref).or_default();
                index
                    .trait_functions_by_trait_and_name
                    .entry(trait_ref)
                    .or_default();
                for item in &trait_data.items {
                    if let AssocItemId::Function(id) = item {
                        let function_ref = FunctionRef {
                            target: trait_ref.target,
                            id: *id,
                        };
                        push_unique(functions, function_ref);
                        if let Some(function_data) = semantic_ir.function_data(function_ref)? {
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

            // Semantic IR has already resolved impl headers into possible `Self` types. The index
            // preserves that optimistic shape: ambiguous impls are attached to every resolved self
            // type, and the later applicability check still decides whether each candidate fits.
            for (impl_ref, impl_data) in semantic_ir.impls(target)? {
                if impl_data.trait_ref.is_none() {
                    for self_ty in &impl_data.resolved_self_tys {
                        push_unique(
                            index.inherent_impls_by_type.entry(*self_ty).or_default(),
                            impl_ref,
                        );
                        for item in &impl_data.items {
                            if let AssocItemId::Function(id) = item {
                                let function_ref = FunctionRef {
                                    target: impl_ref.target,
                                    id: *id,
                                };
                                let Some(function_data) =
                                    semantic_ir.function_data(function_ref)?
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

    pub(crate) fn inherent_functions_for_type(
        &self,
        semantic_ir: &SemanticIrReadTxn<'_>,
        ty: TypeDefRef,
    ) -> Result<Vec<FunctionRef>, PackageStoreError> {
        let mut functions = Vec::new();
        let Some(impl_refs) = self.inherent_impls_by_type.get(&ty) else {
            return Ok(functions);
        };

        // Store impl ids, not function ids, because function lists belong to impl item data. This
        // keeps the index compact while still avoiding the expensive global impl search.
        for impl_ref in impl_refs {
            let Some(data) = semantic_ir.impl_data(*impl_ref)? else {
                continue;
            };

            for item in &data.items {
                if let AssocItemId::Function(id) = item {
                    push_unique(
                        &mut functions,
                        FunctionRef {
                            target: impl_ref.target,
                            id: *id,
                        },
                    );
                }
            }
        }

        Ok(functions)
    }

    pub(crate) fn inherent_functions_for_type_and_name(
        &self,
        ty: TypeDefRef,
        name: &str,
    ) -> &[FunctionRef] {
        // Dot lookup almost always starts with the method name already known. Keeping the name as
        // part of the key lets us avoid checking receiver applicability for unrelated methods.
        self.inherent_functions_by_type_and_name
            .get(&ty)
            .and_then(|functions_by_name| functions_by_name.get(name))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn trait_impls_for_type(&self, ty: TypeDefRef) -> &[TraitImplRef] {
        self.trait_impls_by_type
            .get(&ty)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn trait_functions(&self, trait_ref: TraitRef) -> Option<&[FunctionRef]> {
        // `None` means the trait was not visible while this index was built. Callers can then fall
        // back to the direct Semantic IR query for cross-subset/offloaded edge cases.
        self.trait_functions_by_trait
            .get(&trait_ref)
            .map(Vec::as_slice)
    }

    pub(crate) fn trait_functions_by_name(
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
