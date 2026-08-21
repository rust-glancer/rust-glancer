//! Persisted lookup indexes for one semantic crate.
//!
//! Method lookup, trait selection, and compiler desugarings repeatedly ask for candidates with the
//! same receiver or trait identity. Each `ItemLookupIndex` is serialized beside its owning
//! `ItemStore`, and contains only declarations from that store. It never copies candidates from
//! visible dependencies into itself.
//!
//! This matters for packages with many Cargo targets: a library's candidates stay in the library
//! index instead of being repeated in every test or example target. `item::query` composes the local
//! and dependency indexes only for the operation that needs them.

use std::collections::HashMap;

use rg_ir_model::{
    AssocItemId, CrateRef, FunctionRef, ImplId, ImplRef, TraitDefRef, TraitId, TraitImplRef,
    TypeDefRef,
};
use rg_std::{MemorySize, Shrink, UniqueVec};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use crate::ItemStore;

/// Receiver- and trait-keyed candidates declared by one semantic crate.
///
/// For example, an index contains `impl Widget { fn draw(...) }` when that impl is declared in the
/// indexed crate. It does not contain a method declared by a dependency merely because that
/// dependency is visible; [`crate::ItemLookupQuery`] brings the two crate-local indexes together at
/// lookup time.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ItemLookupIndex {
    // Method lookup starts from a receiver type. These maps let callers jump directly to impls
    // whose already-resolved `Self` type mentions that receiver, instead of re-scanning all impls.
    pub(crate) inherent_impls_by_type: HashMap<TypeDefRef, UniqueVec<ImplRef>>,
    pub(crate) inherent_functions_by_type_and_name:
        HashMap<TypeDefRef, HashMap<Name, UniqueVec<FunctionRef>>>,
    pub(crate) structural_inherent_impls: UniqueVec<ImplRef>,
    pub(crate) trait_impls_by_type: HashMap<TypeDefRef, UniqueVec<IndexedTraitImplRef>>,
    pub(crate) trait_impls_by_trait: HashMap<TraitDefRef, UniqueVec<IndexedImplRef>>,
    // Trait impl lookup produces trait identities first; this cache then expands each trait into
    // its associated function declarations without reopening the trait item every time.
    pub(crate) trait_functions_by_trait: HashMap<TraitDefRef, UniqueVec<FunctionRef>>,
    pub(crate) trait_functions_by_trait_and_name:
        HashMap<TraitDefRef, HashMap<Name, UniqueVec<FunctionRef>>>,
}

impl ItemLookupIndex {
    /// Builds the declaration-local candidate tables for one semantic crate.
    ///
    /// Impl header facts must already be resolved before this runs, because receiver and trait
    /// keys are derived from those facts. Visibility composition belongs to
    /// [`crate::ItemLookupQuery`] and is intentionally not performed by this constructor.
    pub fn build_from_store(store: &ItemStore) -> Self {
        let mut index = Self::default();
        index.extend_from_store(store);
        index
    }

    /// Count retained candidate references across every declaration-local lookup table.
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
                .trait_impls_by_type
                .values()
                .map(UniqueVec::len)
                .sum::<usize>()
            + self
                .trait_impls_by_trait
                .values()
                .map(UniqueVec::len)
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

    fn extend_from_store(&mut self, store: &ItemStore) {
        // Trait methods are independent of a receiver type, so cache them by trait before
        // processing impls that later point back to these traits.
        for (trait_ref, trait_data) in store.traits_with_refs() {
            let functions = self.trait_functions_by_trait.entry(trait_ref).or_default();
            self.trait_impls_by_trait.entry(trait_ref).or_default();
            self.trait_functions_by_trait_and_name
                .entry(trait_ref)
                .or_default();
            for item in &trait_data.items {
                if let AssocItemId::Function(id) = item {
                    let function_ref = FunctionRef {
                        origin: trait_ref.origin,
                        id: *id,
                    };
                    functions.push(function_ref);
                    if let Some(function_data) = store.function_data(*id) {
                        self.trait_functions_by_trait_and_name
                            .entry(trait_ref)
                            .or_default()
                            .entry(function_data.name.clone())
                            .or_default()
                            .push(function_ref);
                    }
                }
            }
        }

        // Item-store lowering has already resolved impl headers into an expected-unique `Self`
        // type. Ambiguous nominal headers are not receiver-indexed. Structural inherent impls
        // need a small side list, while trait impls remain discoverable through their implemented
        // trait and are partitioned by canonical `Self` shape on demand.
        for (impl_ref, impl_data) in store.impls_with_refs() {
            if impl_data.trait_ref.is_none() {
                if impl_data.resolved_self_ty.is_empty() {
                    // Inherent impls for shaped builtin types, such as `impl<T> [T]`, do not have
                    // a nominal receiver key. Keep them in a small side list so structural method
                    // lookup does not scan every visible impl.
                    self.structural_inherent_impls.push(impl_ref);
                }

                if let Some(self_ty) = impl_data.resolved_self_ty.as_option() {
                    self.inherent_impls_by_type
                        .entry(*self_ty)
                        .or_default()
                        .push(impl_ref);
                    for item in &impl_data.items {
                        if let AssocItemId::Function(id) = item {
                            let function_ref = FunctionRef {
                                origin: impl_ref.origin,
                                id: *id,
                            };
                            let Some(function_data) = store.function_data(*id) else {
                                continue;
                            };
                            self.inherent_functions_by_type_and_name
                                .entry(*self_ty)
                                .or_default()
                                .entry(function_data.name.clone())
                                .or_default()
                                .push(function_ref);
                        }
                    }
                }
            } else {
                let Some(trait_ref) = impl_data.resolved_trait_ref.as_option() else {
                    continue;
                };
                let trait_impl = TraitImplRef {
                    impl_ref,
                    trait_ref: *trait_ref,
                };

                // Structural and blanket impls may not have a nominal receiver key, but trait
                // selection starts from the implemented trait and partitions these canonical
                // headers by their top-level `Self` shape later.
                self.trait_impls_by_trait
                    .entry(*trait_ref)
                    .or_default()
                    .push(IndexedImplRef::from_crate(impl_ref));

                if let Some(self_ty) = impl_data.resolved_self_ty.as_option() {
                    self.trait_impls_by_type
                        .entry(*self_ty)
                        .or_default()
                        .push(IndexedTraitImplRef::from_crate(trait_impl));
                }
            }
        }
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
    fn from_crate(impl_ref: ImplRef) -> Self {
        Self {
            crate_ref: impl_ref
                .origin
                .as_crate_ref()
                .expect("semantic item-store impl should have a crate origin"),
            id: impl_ref.id,
        }
    }

    pub(crate) fn expand(self) -> ImplRef {
        ImplRef::new(rg_ir_model::DefMapRef::Crate(self.crate_ref), self.id)
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
    fn from_crate(trait_impl: TraitImplRef) -> Self {
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
            trait_ref: TraitDefRef::new(
                rg_ir_model::DefMapRef::Crate(self.trait_crate),
                self.trait_id,
            ),
        }
    }
}
