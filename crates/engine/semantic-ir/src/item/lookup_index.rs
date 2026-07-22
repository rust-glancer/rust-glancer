//! Precomputed lookup indexes over semantic-shaped item stores.
//!
//! Method lookup, trait selection, and compiler desugarings ask the same receiver-, trait-, and
//! language-identity questions many times. This index pays the visible-store scan once and lets
//! later queries jump straight to plausible candidates while preserving ordinary item-store reads
//! for the final declarations.

use std::collections::HashMap;

use rg_def_map::{CrateData, DefMapSource, PackageSlot};
use rg_ir_model::{
    AssocItemId, DefMapRef, FunctionRef, ImplRef, SemanticItemRef, TraitDefRef, TraitImplRef,
    TypeAliasRef, TypeDefId, TypeDefRef,
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
    trait_impls_by_type: HashMap<TypeDefRef, UniqueVec<TraitImplRef>>,
    trait_impls_by_trait: HashMap<TraitDefRef, UniqueVec<TraitImplRef>>,
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

    /// Encode the body-independent crate facts that can change a lookup index.
    ///
    /// A complete index combines stores visible from one use-site crate. Dirty analysis can reuse
    /// that index when this key is unchanged for every rebuilt crate; dependencies outside the
    /// rebuild keep their saved stores. The key includes the local indexed declarations together
    /// with external roots and the selected prelude, which determine the visible store set.
    ///
    /// Hash maps are projected into sorted vectors before encoding. Candidate vectors keep their
    /// construction order because that order is part of lookup behavior.
    pub fn cache_key(crate_data: &CrateData, store: &ItemStore) -> anyhow::Result<Vec<u8>> {
        fn origin_sort_key(origin: DefMapRef) -> (u8, usize, usize, usize) {
            match origin {
                DefMapRef::Crate(crate_ref) => (0, crate_ref.package.0, crate_ref.crate_id.0, 0),
                DefMapRef::Body(body) => (
                    1,
                    body.crate_ref.package.0,
                    body.crate_ref.crate_id.0,
                    body.body.0,
                ),
            }
        }

        fn type_def_sort_key(ty: TypeDefRef) -> ((u8, usize, usize, usize), u8, usize) {
            let (kind, id) = match ty.id {
                TypeDefId::Struct(id) => (0, id.0),
                TypeDefId::Enum(id) => (1, id.0),
                TypeDefId::Union(id) => (2, id.0),
            };
            (origin_sort_key(ty.origin), kind, id)
        }

        fn trait_sort_key(trait_ref: TraitDefRef) -> ((u8, usize, usize, usize), usize) {
            (origin_sort_key(trait_ref.origin), trait_ref.id.0)
        }

        // Fill the local part with the same code used by the runtime index. This prevents the cache
        // key from drifting away from the data it validates.
        let mut index = Self::default();
        index.extend_from_store(store);
        let Self {
            visible_packages,
            lang_items,
            inherent_impls_by_type,
            inherent_functions_by_type_and_name,
            structural_inherent_impls,
            trait_impls_by_type,
            trait_impls_by_trait,
            trait_functions_by_trait,
            trait_functions_by_trait_and_name,
        } = index;
        debug_assert!(
            visible_packages.is_empty(),
            "a local item-store index should not contain visibility packages"
        );

        // The local item store does not own visibility edges. External roots and the prelude still
        // belong in the key because changing either can select a different set of contributing
        // stores when the complete index is rebuilt.
        let mut extern_roots = crate_data
            .extern_prelude()
            .iter()
            .map(|(name, module)| (name.clone(), *module))
            .collect::<Vec<_>>();
        extern_roots.sort_by(|(left, _), (right, _)| left.cmp(right));

        // Hash-map order must not decide whether two equivalent snapshots can share an index.
        // Sort each map after converting it to vectors, but keep candidates in their lookup order.
        let mut inherent_impls_by_type = inherent_impls_by_type.into_iter().collect::<Vec<_>>();
        inherent_impls_by_type.sort_by_key(|(ty, _)| type_def_sort_key(*ty));

        let mut inherent_functions_by_type_and_name = inherent_functions_by_type_and_name
            .into_iter()
            .map(|(ty, functions)| {
                let mut functions = functions.into_iter().collect::<Vec<_>>();
                functions.sort_by(|(left, _), (right, _)| left.cmp(right));
                (ty, functions)
            })
            .collect::<Vec<_>>();
        inherent_functions_by_type_and_name.sort_by_key(|(ty, _)| type_def_sort_key(*ty));

        let mut trait_impls_by_type = trait_impls_by_type.into_iter().collect::<Vec<_>>();
        trait_impls_by_type.sort_by_key(|(ty, _)| type_def_sort_key(*ty));

        let mut trait_impls_by_trait = trait_impls_by_trait.into_iter().collect::<Vec<_>>();
        trait_impls_by_trait.sort_by_key(|(trait_ref, _)| trait_sort_key(*trait_ref));

        let mut trait_functions_by_trait = trait_functions_by_trait.into_iter().collect::<Vec<_>>();
        trait_functions_by_trait.sort_by_key(|(trait_ref, _)| trait_sort_key(*trait_ref));

        let mut trait_functions_by_trait_and_name = trait_functions_by_trait_and_name
            .into_iter()
            .map(|(trait_ref, functions)| {
                let mut functions = functions.into_iter().collect::<Vec<_>>();
                functions.sort_by(|(left, _), (right, _)| left.cmp(right));
                (trait_ref, functions)
            })
            .collect::<Vec<_>>();
        trait_functions_by_trait_and_name.sort_by_key(|(trait_ref, _)| trait_sort_key(*trait_ref));

        // This tuple is an encoding detail rather than another semantic entity. Its order is the
        // cache contract: local index facts first, then the edges that select visible stores.
        let key = (
            (
                lang_items,
                inherent_impls_by_type,
                inherent_functions_by_type_and_name,
                structural_inherent_impls,
                trait_impls_by_type,
                trait_impls_by_trait,
                trait_functions_by_trait,
                trait_functions_by_trait_and_name,
            ),
            extern_roots,
            crate_data.prelude(),
        );
        wincode::config::serialize(&key, wincode::config::Configuration::default())
            .map_err(|error| anyhow::anyhow!("{error}"))
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
                    .push(trait_impl);

                if let Some(self_ty) = impl_data.resolved_self_ty.as_option() {
                    self.trait_impls_by_type
                        .entry(*self_ty)
                        .or_default()
                        .push(trait_impl);
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
            impls.extend(trait_impls.iter().map(|trait_impl| trait_impl.impl_ref));
        }
        impls
    }

    /// Returns impl blocks indexed for an implemented trait.
    pub fn impls_for_trait(&self, trait_ref: TraitDefRef) -> UniqueVec<ImplRef> {
        self.trait_impls_by_trait
            .get(&trait_ref)
            .into_iter()
            .flat_map(|trait_impls| trait_impls.iter())
            .map(|trait_impl| trait_impl.impl_ref)
            .collect()
    }

    /// Returns trait impl candidates indexed for a receiver type.
    pub fn trait_impls_for_type(&self, ty: TypeDefRef) -> Option<&UniqueVec<TraitImplRef>> {
        self.trait_impls_by_type.get(&ty)
    }

    /// Returns trait impl candidates indexed by the implemented trait.
    pub fn trait_impls_for_trait(
        &self,
        trait_ref: TraitDefRef,
    ) -> Option<&UniqueVec<TraitImplRef>> {
        self.trait_impls_by_trait.get(&trait_ref)
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
