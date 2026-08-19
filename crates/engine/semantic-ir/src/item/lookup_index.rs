//! Precomputed lookup indexes over semantic-shaped item stores.
//!
//! Method lookup, trait selection, and compiler desugarings ask the same receiver-, trait-, and
//! language-identity questions many times. This index pays the visible-store scan once and lets
//! later queries jump straight to plausible candidates while preserving ordinary item-store reads
//! for the final declarations.

use std::collections::HashMap;

use rg_def_map::{DefMapSource, PackageSlot};
use rg_ir_model::{
    AssocItemId, CrateRef, FunctionRef, ImplId, ImplRef, SemanticItemRef, TraitDefRef, TraitId,
    TraitImplRef, TypeAliasRef, TypeDefRef,
};
use rg_item_tree::LangItem;
use rg_std::{MemorySize, Shrink, UniqueVec};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    CrateItemQuery, ItemStore, ItemStoreQuery, ItemStoreSource, item::lang_item::VisibleLangItems,
};

/// Candidate tables for receiver, trait, and language-item lookups visible from one crate.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ItemLookupIndex {
    // Building this index reads semantic stores from these packages. Keep the sorted set beside
    // the persisted index so a later dirty rebuild can warm the same declarations without asking
    // offloaded DefMaps to discover the visible stores again.
    visible_packages: Vec<PackageSlot>,
    // Language items are also visibility-scoped and are queried from the hottest receiver and
    // callable paths. Merge them while the visible stores are already being scanned here.
    lang_items: VisibleLangItems,
    // Method lookup starts from a receiver type. These maps let callers jump directly to impls
    // whose already-resolved `Self` type mentions that receiver, instead of re-scanning all impls.
    inherent_impls_by_type: HashMap<TypeDefRef, UniqueVec<ImplRef>>,
    inherent_functions_by_type_and_name: HashMap<TypeDefRef, HashMap<Name, UniqueVec<FunctionRef>>>,
    structural_inherent_impls: UniqueVec<ImplRef>,
    trait_impls_by_type: HashMap<TypeDefRef, UniqueVec<IndexedTraitImplRef>>,
    trait_impls_by_trait: HashMap<TraitDefRef, UniqueVec<IndexedImplRef>>,
    // Trait impl lookup produces trait identities first; this cache then expands each trait into
    // its associated function declarations without reopening the trait item every time.
    trait_functions_by_trait: HashMap<TraitDefRef, UniqueVec<FunctionRef>>,
    trait_functions_by_trait_and_name: HashMap<TraitDefRef, HashMap<Name, UniqueVec<FunctionRef>>>,
}

impl ItemLookupIndex {
    /// Builds an index from the stores visible from one use-site crate.
    pub fn build_from<'item, D, I>(
        crate_items: &CrateItemQuery<'item, D, I>,
    ) -> Result<Self, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'item>,
    {
        let mut index = Self::default();

        // Pay the visibility-scoped store scan once before body and editor queries start asking
        // repeated receiver, trait, and language-item questions. Record which packages supplied
        // those stores so cache-backed reuse can warm exactly the same declarations.
        let stores = crate_items.visible_stores()?;
        let mut visible_packages = UniqueVec::new();
        for store in &stores {
            visible_packages.push(store.crate_ref().package);
        }
        index.visible_packages = visible_packages.into_vec();
        index.visible_packages.sort_by_key(|package| package.0);
        for store in stores {
            index.extend_from_store(store);
        }

        Ok(index)
    }

    /// Packages whose semantic stores contributed to this use-site index.
    pub fn visible_packages(&self) -> &[PackageSlot] {
        &self.visible_packages
    }

    fn extend_from_store(&mut self, store: &ItemStore) {
        for lang_item in LangItem::ALL {
            self.lang_items
                .merge_prefer_existing(lang_item, store.lang_item(lang_item));
        }

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

    /// Returns the exact visible trait carrying one compiler language identity.
    ///
    /// A missing or ambiguous declaration produces `None` rather than guessing from names.
    pub fn lang_trait(&self, lang_item: LangItem) -> Option<TraitDefRef> {
        let SemanticItemRef::Trait(trait_ref) = self.lang_items.target(lang_item)? else {
            return None;
        };
        Some(trait_ref)
    }

    /// Returns the exact visible function carrying one compiler language identity.
    ///
    /// A missing or ambiguous declaration produces `None` rather than guessing from names.
    pub fn lang_function(&self, lang_item: LangItem) -> Option<FunctionRef> {
        let SemanticItemRef::Function(function_ref) = self.lang_items.target(lang_item)? else {
            return None;
        };
        Some(function_ref)
    }

    /// Returns the exact visible type alias carrying one compiler language identity.
    ///
    /// A missing or ambiguous declaration produces `None` rather than guessing from names.
    pub fn lang_type_alias(&self, lang_item: LangItem) -> Option<TypeAliasRef> {
        let SemanticItemRef::TypeAlias(type_alias_ref) = self.lang_items.target(lang_item)? else {
            return None;
        };
        Some(type_alias_ref)
    }

    /// Expands indexed inherent impls to their function items through the caller's query source.
    pub fn inherent_functions_for_type<'item, S>(
        &self,
        item_query: &ItemStoreQuery<'item, S>,
        ty: TypeDefRef,
    ) -> Result<UniqueVec<FunctionRef>, S::Error>
    where
        S: ItemStoreSource<'item>,
    {
        let mut functions = UniqueVec::new();
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
                    functions.push(FunctionRef {
                        origin: impl_ref.origin,
                        id: *id,
                    });
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
    ) -> Option<&UniqueVec<FunctionRef>> {
        // Dot lookup almost always starts with the method name already known. Keeping the name as
        // part of the key lets callers avoid checking receiver applicability for unrelated methods.
        self.inherent_functions_by_type_and_name
            .get(&ty)
            .and_then(|functions_by_name| functions_by_name.get(name))
    }

    /// Returns inherent impls whose `Self` type needs structural matching instead of a type key.
    pub fn structural_inherent_impls(&self) -> &UniqueVec<ImplRef> {
        &self.structural_inherent_impls
    }

    /// Returns inherent impls indexed for a receiver type.
    pub fn inherent_impls_for_type(&self, ty: TypeDefRef) -> UniqueVec<ImplRef> {
        self.inherent_impls_by_type
            .get(&ty)
            .cloned()
            .unwrap_or_default()
    }

    /// Returns impl blocks indexed for a receiver type, including inherent and trait impls.
    pub fn impls_for_type(&self, ty: TypeDefRef) -> UniqueVec<ImplRef> {
        let mut impls = self.inherent_impls_for_type(ty);
        if let Some(trait_impls) = self.trait_impls_by_type.get(&ty) {
            impls.extend(
                trait_impls
                    .iter()
                    .map(|trait_impl| trait_impl.expand().impl_ref),
            );
        }
        impls
    }

    /// Returns impl blocks indexed for an implemented trait.
    pub fn impls_for_trait(&self, trait_ref: TraitDefRef) -> UniqueVec<ImplRef> {
        self.trait_impls_by_trait
            .get(&trait_ref)
            .into_iter()
            .flat_map(|trait_impls| trait_impls.iter())
            .map(|impl_ref| impl_ref.expand())
            .collect()
    }

    /// Returns trait impl candidates indexed for a receiver type.
    pub fn trait_impls_for_type(&self, ty: TypeDefRef) -> UniqueVec<TraitImplRef> {
        self.trait_impls_by_type
            .get(&ty)
            .into_iter()
            .flat_map(|trait_impls| trait_impls.iter())
            .map(|trait_impl| trait_impl.expand())
            .collect()
    }

    /// Returns trait impl candidates indexed by the implemented trait.
    pub fn trait_impls_for_trait(
        &self,
        trait_ref: TraitDefRef,
    ) -> Option<impl ExactSizeIterator<Item = TraitImplRef> + Clone + '_> {
        self.trait_impls_by_trait.get(&trait_ref).map(move |impls| {
            impls.iter().map(move |impl_ref| TraitImplRef {
                impl_ref: impl_ref.expand(),
                trait_ref,
            })
        })
    }

    /// Returns trait-declared functions if the trait was visible when the index was built.
    pub fn trait_functions(&self, trait_ref: TraitDefRef) -> Option<&UniqueVec<FunctionRef>> {
        self.trait_functions_by_trait.get(&trait_ref)
    }

    /// Returns same-name trait functions if the trait was visible when the index was built.
    pub fn trait_functions_by_name(
        &self,
        trait_ref: TraitDefRef,
        name: &str,
    ) -> Option<IndexedTraitFunctions<'_>> {
        // `Some(&[])` is meaningful: the trait is indexed and has no function with this name, so
        // callers can skip the trait-impl applicability check entirely for this method lookup.
        let functions_by_name = self.trait_functions_by_trait_and_name.get(&trait_ref)?;
        Some(IndexedTraitFunctions {
            functions: functions_by_name.get(name),
        })
    }
}

/// Crate-only impl identity retained inside a crate-visible lookup index.
///
/// A general [`ImplRef`] also has to represent body-local declarations, which makes its
/// [`rg_ir_model::DefMapRef`] origin larger. Entries collected from semantic item stores cannot be
/// body-local, so retaining the crate directly avoids paying for that unused variant millions of
/// times. Query methods expand this back to the ordinary public identity at their boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
struct IndexedImplRef {
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

    fn expand(self) -> ImplRef {
        ImplRef::new(rg_ir_model::DefMapRef::Crate(self.crate_ref), self.id)
    }
}

/// Compact trait impl identity used where the trait is not already present as the map key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
struct IndexedTraitImplRef {
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

    fn expand(self) -> TraitImplRef {
        TraitImplRef {
            impl_ref: self.impl_ref.expand(),
            trait_ref: TraitDefRef::new(
                rg_ir_model::DefMapRef::Crate(self.trait_crate),
                self.trait_id,
            ),
        }
    }
}

pub struct IndexedTraitFunctions<'a> {
    functions: Option<&'a UniqueVec<FunctionRef>>,
}

impl<'a> IndexedTraitFunctions<'a> {
    pub fn is_empty(&self) -> bool {
        self.functions.is_none_or(UniqueVec::is_empty)
    }

    pub fn functions(&self) -> Option<&'a UniqueVec<FunctionRef>> {
        self.functions
    }
}
