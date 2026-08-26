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
    // Inherent lookup starts from a receiver type. These maps jump directly to impls whose
    // already-resolved `Self` type mentions that receiver.
    pub(crate) inherent_impls_by_type: HashMap<TypeDefRef, UniqueVec<ImplRef>>,
    pub(crate) inherent_functions_by_type_and_name:
        HashMap<TypeDefRef, HashMap<Name, UniqueVec<FunctionRef>>>,
    pub(crate) structural_inherent_impls: UniqueVec<ImplRef>,
    // Implementation navigation and qualified paths still ask which impls mention one nominal
    // type. Trait-item lookup uses the trait-keyed map below instead.
    pub(crate) trait_impls_by_type: HashMap<TypeDefRef, UniqueVec<IndexedTraitImplRef>>,
    pub(crate) trait_impls_by_trait: HashMap<TraitDefRef, UniqueVec<IndexedImplRef>>,
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

    fn extend_from_store(&mut self, store: &ItemStore) {
        // Record trait declaration surfaces before processing impls. For `value.convert()`, the
        // name `convert` should select `Convert` before its impl headers are compared with value's
        // type. `value./* completion */` instead starts from every trait containing a function.
        for (trait_ref, trait_data) in store.traits_with_refs() {
            if !trait_data.items.is_empty() {
                self.traits_with_associated_items.push(trait_ref);
            }
            let functions = self.trait_functions_by_trait.entry(trait_ref).or_default();
            self.trait_impls_by_trait.entry(trait_ref).or_default();
            self.trait_functions_by_trait_and_name
                .entry(trait_ref)
                .or_default();
            for item in &trait_data.items {
                match item {
                    AssocItemId::Function(id) => {
                        let function_ref = FunctionRef {
                            origin: trait_ref.origin,
                            id: *id,
                        };
                        functions.push(function_ref);
                        self.traits_with_functions.push(trait_ref);
                        if let Some(function_data) = store.function_data(*id) {
                            self.traits_by_item_name
                                .entry(function_data.name.clone())
                                .or_default()
                                .functions
                                .push(trait_ref);
                            self.trait_functions_by_trait_and_name
                                .entry(trait_ref)
                                .or_default()
                                .entry(function_data.name.clone())
                                .or_default()
                                .push(function_ref);
                        }
                    }
                    AssocItemId::Const(id) => {
                        if let Some(const_data) = store.const_data(*id) {
                            self.traits_by_item_name
                                .entry(const_data.name.clone())
                                .or_default()
                                .consts
                                .push(trait_ref);
                        }
                    }
                    // Associated type completion uses the trait-wide surface above. Named type
                    // projection follows written trait bounds, so it needs no reverse name index.
                    AssocItemId::TypeAlias(_) => {}
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
