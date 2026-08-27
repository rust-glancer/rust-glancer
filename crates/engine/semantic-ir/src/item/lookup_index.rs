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
    AssocItemId, CrateRef, DefMapRef, FunctionRef, ImplId, ImplRef, Mutability, PrimitiveTy,
    SemanticItemRef, TraitDefRef, TraitId, TraitImplRef, TypeDefRef,
};
use rg_item_tree::LangItem;
use rg_std::{ExpectedUnique, MemorySize, Shrink, UniqueVec};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use crate::{ItemStore, item::lang_item::LangItemIndex};

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

/// Candidate tables and compiler language identities declared by one semantic crate.
///
/// For example, an index contains `impl Widget { fn draw(...) }` when that impl is declared in the
/// indexed crate. It does not contain a method declared by a dependency merely because that
/// dependency is visible; [`crate::ItemLookupQuery`] brings the two crate-local indexes together at
/// lookup time. Sparse language-item entries live here for the same reason: discovering a compiler
/// identity should not require loading the crate's complete declaration store.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ItemLookupIndex {
    // Language items are lookup metadata too. Keeping this sparse index here lets a use-site query
    // assemble compiler identities without restoring every visible declaration store.
    lang_items: LangItemIndex,
    // Inherent lookup starts from a receiver type. These maps jump directly to impls whose
    // already-resolved `Self` type mentions that receiver.
    pub(crate) inherent_impls_by_type: HashMap<TypeDefRef, UniqueVec<ImplRef>>,
    pub(crate) inherent_functions_by_type_and_name:
        HashMap<TypeDefRef, HashMap<Name, UniqueVec<FunctionRef>>>,
    pub(crate) structural_inherent_impls: UniqueVec<ImplRef>,
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

impl ItemLookupIndex {
    /// Builds the declaration-local candidate tables for one semantic crate.
    ///
    /// Production lowering supplies the conservative receiver heads resolved by the semantic
    /// header pass. Synthetic stores without a definition resolver can pass an empty map; their
    /// trait impls then enter the fallback lane. Visibility composition belongs to
    /// [`crate::ItemLookupQuery`] and is intentionally not performed here.
    pub fn build_from_store(
        store: &ItemStore,
        self_heads: &HashMap<ImplRef, TraitImplSelfHead>,
    ) -> Self {
        let mut index = Self {
            lang_items: store.lang_items().clone(),
            ..Self::default()
        };
        index.extend_from_store(store, self_heads);
        index
    }

    /// Projects one crate-local language-item entry into its project-wide semantic identity.
    pub(crate) fn lang_item(
        &self,
        crate_ref: CrateRef,
        lang_item: LangItem,
    ) -> ExpectedUnique<SemanticItemRef> {
        self.lang_items
            .target(lang_item, DefMapRef::Crate(crate_ref))
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

    fn extend_from_store(
        &mut self,
        store: &ItemStore,
        self_heads: &HashMap<ImplRef, TraitImplSelfHead>,
    ) {
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
        // need a small side list. Trait impls use the conservative source-level head computed by
        // the header pass, so receiver lookup does not need to reopen every canonical header.
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

                // `impl Marker for [u8]` enters the slice lane, while `impl<T> Marker for T` and
                // alias-headed impls stay in the fallback lane. Both are still grouped under the
                // selected trait before the type layer performs exact matching and proof.
                self.trait_impls_by_trait
                    .entry(*trait_ref)
                    .or_default()
                    .push(
                        IndexedImplRef::from_crate(impl_ref),
                        self_heads.get(&impl_ref).copied(),
                    );

                if let Some(self_ty) = impl_data.resolved_self_ty.as_option() {
                    self.trait_impls_by_type
                        .entry(*self_ty)
                        .or_default()
                        .push(IndexedTraitImplRef::from_crate(trait_impl));
                }
            }
        }
    }

    /// Return one trait's direct receiver-head candidates followed by conservative fallbacks.
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
    /// `None` asks only for fallbacks. Closure and function-item receivers use that lane because a
    /// concrete source impl cannot name their body-owned identity.
    pub(crate) fn trait_impl_candidates_for_self_head(
        &self,
        trait_ref: TraitDefRef,
        self_head: Option<TraitImplSelfHead>,
    ) -> Option<UniqueVec<IndexedImplRef>> {
        let indexed = self.trait_impls_by_trait.get(&trait_ref)?;
        let mut impls = UniqueVec::new();

        match self_head {
            Some(TraitImplSelfHead::Adt(type_def)) => {
                if let Some(candidates) = self.trait_impls_by_type.get(&type_def) {
                    impls.extend(candidates.iter().filter_map(|candidate| {
                        (candidate.expand().trait_ref == trait_ref).then_some(candidate.impl_ref)
                    }));
                }
            }
            Some(self_head) => {
                if let Some(candidates) = indexed.direct_by_self_head.get(&self_head) {
                    impls.extend(candidates.iter().copied());
                }
            }
            None => {}
        }
        impls.extend(indexed.fallbacks.iter().copied());
        Some(impls)
    }
}

/// All impls of one trait plus the receiver lanes used by native candidate discovery.
///
/// For `impl Marker for u32`, `impl<T> Marker for [T]`, and `impl<T> Marker for T`, `all` contains
/// all three declarations, `direct_by_self_head` has primitive and slice entries, and `fallbacks`
/// contains only the blanket impl. This lets a known `u32` receiver avoid reopening the slice impl
/// while still considering the blanket one.
///
/// Nominal direct impls already live in `trait_impls_by_type`, so this value duplicates only
/// structural and fallback impl identities. The complete list remains necessary for Chalk roots,
/// implementation navigation, and unresolved receiver fallbacks.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub(crate) struct IndexedTraitImpls {
    pub(crate) all: UniqueVec<IndexedImplRef>,
    fallbacks: UniqueVec<IndexedImplRef>,
    direct_by_self_head: HashMap<TraitImplSelfHead, UniqueVec<IndexedImplRef>>,
}

impl IndexedTraitImpls {
    fn push(&mut self, impl_ref: IndexedImplRef, self_head: Option<TraitImplSelfHead>) {
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
