//! Builds one use-site lookup view from persisted crate-local indexes.
//!
//! Each semantic crate persists only the candidates declared in that crate. A lookup from a real
//! Rust use site still needs two parts:
//!
//! - declarations from the use-site crate itself;
//! - declarations from the dependency crates visible through DefMap.
//!
//! Keeping those parts separate is important for packages with many targets. Suppose two
//! integration tests have different local declarations but the same dependencies:
//!
//! ```text
//! ItemLookupQueryCache (one build or analysis operation)
//! └── [dep_a, dep_b] -> shared dependency lookup results
//!                              ↑                 ↑
//! ItemLookupQuery(test_a)      │    ItemLookupQuery(test_b)
//! ├── local: test_a index      │    ├── local: test_b index
//! └── dependencies ────────────┘    └── dependencies ────────────┘
//! ```
//!
//! A lookup such as `(Widget, "draw")` reads candidates from the test's local index, then appends
//! the cached or newly computed union from `dep_a` and `dep_b`. Local candidates never enter the
//! shared dependency result, so one test cannot leak declarations into another. The operation cache
//! and all memoized unions are dropped after the build or analysis request.
//!
//! Language items take a smaller, separate path. There are only a fixed number of them, and the
//! first visible declaration matters, so each query resolves them eagerly while it still has the
//! complete ordered index list.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use rg_def_map::DefMapSource;
use rg_ir_model::{
    AssocItemId, CrateRef, FunctionRef, ImplRef, SemanticItemRef, TraitDefRef, TraitImplRef,
    TypeAliasRef, TypeDefRef,
};
use rg_item_tree::LangItem;
use rg_std::{CancellationToken, Cancelled, OperationError, UniqueVec};
use rg_text::Name;

use super::{CrateItemQuery, ItemLookupIndexSource, ItemStoreQuery, ItemStoreSource};
use crate::{
    ItemLookupIndex, TraitImplSelfHead,
    item::{TraitItemTraitRefs, lang_item::VisibleLangItems},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TraitItemKind {
    Function,
    Const,
}

impl TraitItemKind {
    fn declaring_traits(self, refs: &TraitItemTraitRefs) -> &UniqueVec<TraitDefRef> {
        match self {
            Self::Function => &refs.functions,
            Self::Const => &refs.consts,
        }
    }
}

// ==============================================================================
// Use-Site Lookup
// ==============================================================================

/// Lookup over the declarations visible from one semantic crate.
///
/// The use-site index remains local to this query. `DependencyLookup` handles the ordered
/// dependency indexes and can share their memoized results with sibling use sites created from the
/// same [`ItemLookupQueryCache`].
#[derive(Debug, Clone)]
pub struct ItemLookupQuery<'item> {
    local_index: &'item ItemLookupIndex,
    dependencies: DependencyLookup<'item>,
    lang_items: VisibleLangItems,
}

impl rg_std::Cancelable for ItemLookupQuery<'_> {
    fn check_cancelled(&self, checkpoint: &'static str) -> Result<(), Cancelled> {
        rg_std::Cancelable::check_cancelled(&self.dependencies, checkpoint)
    }
}

impl<'item> ItemLookupQuery<'item> {
    /// Build a standalone use-site query.
    ///
    /// This creates a private operation cache. Use [`Self::build_with_cache`] when several sibling
    /// queries should reuse dependency results.
    pub fn build_from<D, I>(
        crate_items: &CrateItemQuery<'item, D, I>,
        cancellation: &rg_std::CancellationToken,
    ) -> Result<Self, rg_std::OperationError<I::Error>>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemLookupIndexSource<'item>,
    {
        Self::build_with_cache(crate_items, &ItemLookupQueryCache::new(), cancellation)
    }

    /// Build one use-site query using an operation cache shared with sibling queries.
    pub fn build_with_cache<D, I>(
        crate_items: &CrateItemQuery<'item, D, I>,
        cache: &ItemLookupQueryCache,
        cancellation: &rg_std::CancellationToken,
    ) -> Result<Self, rg_std::OperationError<I::Error>>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemLookupIndexSource<'item>,
    {
        let mut indexes = crate_items.visible_indexes(cancellation)?;

        // 1. Resolve the small fixed language-item set while the complete visibility order is
        // available. The first visible declaration wins, including an ambiguous declaration.
        let mut lang_items = VisibleLangItems::default();
        for (crate_ref, index) in &indexes {
            rg_std::check_cancel!(cancellation, "visible language items");
            for lang_item in LangItem::ALL {
                lang_items.merge_prefer_existing(lang_item, index.lang_item(*crate_ref, lang_item));
            }
        }

        // 2. Remove the use-site index from the dependency set. It is a cheap local overlay and
        // must not become part of results shared by sibling integration-test targets.
        let local_position = indexes
            .iter()
            .position(|(crate_ref, _)| *crate_ref == crate_items.use_site())
            .expect("visible indexes should contain the use-site crate");
        let (_, local_index) = indexes.remove(local_position);
        let dependency_crates = indexes
            .iter()
            .map(|(crate_ref, _)| *crate_ref)
            .collect::<Vec<_>>();
        let dependency_indexes = indexes
            .into_iter()
            .map(|(_, index)| index)
            .collect::<Vec<_>>();

        // 3. The ordered dependency crate list identifies reusable results. The indexes stay
        // borrowed by this query; only compact candidate identities enter the operation cache.
        let dependencies =
            cache.dependencies(dependency_crates, dependency_indexes, cancellation.clone());

        Ok(Self {
            local_index,
            dependencies,
            lang_items,
        })
    }

    /// Returns the exact visible trait carrying one compiler language identity.
    pub fn lang_trait(&self, lang_item: LangItem) -> Option<TraitDefRef> {
        let SemanticItemRef::Trait(trait_ref) = self.lang_items.target(lang_item)? else {
            return None;
        };
        Some(trait_ref)
    }

    /// Returns the exact visible function carrying one compiler language identity.
    pub fn lang_function(&self, lang_item: LangItem) -> Option<FunctionRef> {
        let SemanticItemRef::Function(function_ref) = self.lang_items.target(lang_item)? else {
            return None;
        };
        Some(function_ref)
    }

    /// Returns the exact visible type alias carrying one compiler language identity.
    pub fn lang_type_alias(&self, lang_item: LangItem) -> Option<TypeAliasRef> {
        let SemanticItemRef::TypeAlias(type_alias_ref) = self.lang_items.target(lang_item)? else {
            return None;
        };
        Some(type_alias_ref)
    }

    // Inherent lookup always puts use-site declarations before dependency declarations. The
    // dependency half performs its own demand caching, while the local half is a direct map read.

    /// Expands visible inherent impls to their function items through the caller's query source.
    pub fn inherent_functions_for_type<'query, S>(
        &self,
        item_query: &ItemStoreQuery<'query, S>,
        ty: TypeDefRef,
    ) -> Result<UniqueVec<FunctionRef>, OperationError<S::Error>>
    where
        S: ItemStoreSource<'query>,
    {
        let mut functions = UniqueVec::new();
        for impl_ref in self.inherent_impls_for_type(ty)? {
            let Some(data) = item_query
                .impl_data(impl_ref)
                .map_err(OperationError::Source)?
            else {
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
    #[rg_std::cancelable("lookup candidates")]
    pub fn inherent_functions_for_type_and_name(
        &self,
        ty: TypeDefRef,
        name: &str,
    ) -> Result<UniqueVec<FunctionRef>, Cancelled> {
        let mut functions = self
            .local_index
            .inherent_functions_by_type_and_name
            .get(&ty)
            .and_then(|by_name| by_name.get(name))
            .cloned()
            .unwrap_or_default();
        self.dependencies.extend(
            &mut functions,
            self.dependencies
                .inherent_functions_for_type_and_name(ty, name)?,
        )?;
        Ok(functions)
    }

    /// Returns inherent impls whose `Self` type needs structural matching.
    #[rg_std::cancelable("lookup candidates")]
    pub fn structural_inherent_impls(&self) -> Result<UniqueVec<ImplRef>, Cancelled> {
        let mut impls = self.local_index.structural_inherent_impls.clone();
        self.dependencies
            .extend(&mut impls, self.dependencies.structural_inherent_impls()?)?;
        Ok(impls)
    }

    /// Returns visible inherent impls indexed for a receiver type.
    #[rg_std::cancelable("lookup candidates")]
    pub fn inherent_impls_for_type(&self, ty: TypeDefRef) -> Result<UniqueVec<ImplRef>, Cancelled> {
        let mut impls = self
            .local_index
            .inherent_impls_by_type
            .get(&ty)
            .cloned()
            .unwrap_or_default();
        self.dependencies
            .extend(&mut impls, self.dependencies.inherent_impls_for_type(ty)?)?;
        Ok(impls)
    }

    /// Returns visible impl blocks indexed for a receiver type.
    pub fn impls_for_type(&self, ty: TypeDefRef) -> impl Iterator<Item = ImplRef> + '_ {
        // Navigation consumes candidates once and may stop midway. Borrow each index as the
        // caller advances instead of first building a workspace-sized union.
        std::iter::once(self.local_index)
            .chain(self.dependencies.indexes.iter().copied())
            .flat_map(move |index| {
                index
                    .inherent_impls_by_type
                    .get(&ty)
                    .into_iter()
                    .flat_map(|impls| impls.iter().copied())
                    .chain(
                        index
                            .trait_impls_by_type
                            .get(&ty)
                            .into_iter()
                            .flat_map(|impls| {
                                impls.iter().map(|candidate| candidate.expand().impl_ref)
                            }),
                    )
            })
    }

    // Trait lookup preserves whether the trait itself was visible. `None` means that no local or
    // dependency index knows the trait; `Some(empty)` means the trait is visible but no candidate
    // matched the narrower question.

    /// Returns visible impl blocks indexed for an implemented trait.
    pub fn impls_for_trait(&self, trait_ref: TraitDefRef) -> impl Iterator<Item = ImplRef> + '_ {
        std::iter::once(self.local_index)
            .chain(self.dependencies.indexes.iter().copied())
            .flat_map(move |index| {
                index
                    .trait_impls_by_trait
                    .get(&trait_ref)
                    .into_iter()
                    .flat_map(|impls| impls.all.iter().map(|candidate| candidate.expand()))
            })
    }

    /// Returns visible trait impl candidates indexed for a receiver type.
    #[rg_std::cancelable("lookup candidates")]
    pub fn trait_impls_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<UniqueVec<TraitImplRef>, Cancelled> {
        let local_candidates = self
            .local_index
            .trait_impls_by_type
            .get(&ty)
            .into_iter()
            .flat_map(|impls| impls.iter())
            .map(|trait_impl| trait_impl.expand());
        let mut impls = UniqueVec::new();
        self.dependencies.extend(&mut impls, local_candidates)?;
        self.dependencies
            .extend(&mut impls, self.dependencies.trait_impls_for_type(ty)?)?;
        Ok(impls)
    }

    /// Returns trait impls, preserving whether the implemented trait was visible.
    #[rg_std::cancelable("lookup candidates")]
    pub fn trait_impls_for_trait(
        &self,
        trait_ref: TraitDefRef,
    ) -> Result<Option<UniqueVec<TraitImplRef>>, Cancelled> {
        let dependency_impls = self.dependencies.trait_impls_for_trait(trait_ref)?;
        let local_impls = self.local_index.trait_impls_by_trait.get(&trait_ref);
        if local_impls.is_none() && dependency_impls.is_none() {
            return Ok(None);
        }

        let local_candidates = local_impls
            .into_iter()
            .flat_map(|impls| impls.all.iter())
            .map(|impl_ref| TraitImplRef {
                impl_ref: impl_ref.expand(),
                trait_ref,
            });
        let mut impls = UniqueVec::new();
        self.dependencies.extend(&mut impls, local_candidates)?;
        self.dependencies
            .extend(&mut impls, dependency_impls.unwrap_or_default())?;
        Ok(Some(impls))
    }

    /// Returns direct receiver-head impls followed by blanket or unresolved-head fallbacks.
    ///
    /// With `impl Marker for u32`, `impl<T> Marker for [T]`, and `impl<T> Marker for T`, a
    /// `Primitive(UnsignedInt(U32))` query returns the direct `u32` impl followed by the blanket
    /// impl. It does not return the slice impl, and it does not prove either returned header.
    ///
    /// `None` as the head asks only for fallbacks. This is used for closure and function-item
    /// receivers, whose concrete identities cannot be named by a source impl header.
    #[rg_std::cancelable("lookup candidates")]
    pub fn trait_impl_candidates_for_self_head(
        &self,
        trait_ref: TraitDefRef,
        self_head: Option<TraitImplSelfHead>,
    ) -> Result<Option<UniqueVec<TraitImplRef>>, Cancelled> {
        let dependency_impls = self
            .dependencies
            .trait_impl_candidates_for_self_head(trait_ref, self_head)?;
        let local_impls = self
            .local_index
            .trait_impl_candidates_for_self_head(trait_ref, self_head);
        if local_impls.is_none() && dependency_impls.is_none() {
            return Ok(None);
        }

        let local_candidates = local_impls
            .into_iter()
            .flatten()
            .map(|impl_ref| TraitImplRef {
                impl_ref: impl_ref.expand(),
                trait_ref,
            });
        let mut impls = UniqueVec::new();
        self.dependencies.extend(&mut impls, local_candidates)?;
        self.dependencies
            .extend(&mut impls, dependency_impls.unwrap_or_default())?;
        Ok(Some(impls))
    }

    /// Returns visible traits that declare at least one function.
    #[rg_std::cancelable("lookup candidates")]
    pub fn traits_with_functions(&self) -> Result<UniqueVec<TraitDefRef>, Cancelled> {
        let mut traits = self.local_index.traits_with_functions.clone();
        self.dependencies
            .extend(&mut traits, self.dependencies.traits_with_functions()?)?;
        Ok(traits)
    }

    /// Returns visible traits that declare at least one associated item.
    #[rg_std::cancelable("lookup candidates")]
    pub fn traits_with_associated_items(&self) -> Result<UniqueVec<TraitDefRef>, Cancelled> {
        let mut traits = self.local_index.traits_with_associated_items.clone();
        self.dependencies.extend(
            &mut traits,
            self.dependencies.traits_with_associated_items()?,
        )?;
        Ok(traits)
    }

    /// Returns visible traits with a function declaration carrying this name.
    #[rg_std::cancelable("lookup candidates")]
    pub fn traits_with_function_name(
        &self,
        name: &str,
    ) -> Result<UniqueVec<TraitDefRef>, Cancelled> {
        self.traits_with_item_name(TraitItemKind::Function, name)
    }

    /// Returns visible traits with an associated const declaration carrying this name.
    #[rg_std::cancelable("lookup candidates")]
    pub fn traits_with_const_name(&self, name: &str) -> Result<UniqueVec<TraitDefRef>, Cancelled> {
        self.traits_with_item_name(TraitItemKind::Const, name)
    }

    #[rg_std::cancelable("lookup candidates")]
    fn traits_with_item_name(
        &self,
        kind: TraitItemKind,
        name: &str,
    ) -> Result<UniqueVec<TraitDefRef>, Cancelled> {
        let mut traits = self
            .local_index
            .traits_by_item_name
            .get(name)
            .map(|refs| kind.declaring_traits(refs).clone())
            .unwrap_or_default();
        self.dependencies.extend(
            &mut traits,
            self.dependencies.traits_with_item_name(kind, name)?,
        )?;
        Ok(traits)
    }

    /// Returns declared functions, preserving whether the trait was visible.
    #[rg_std::cancelable("lookup candidates")]
    pub fn trait_functions(
        &self,
        trait_ref: TraitDefRef,
    ) -> Result<Option<UniqueVec<FunctionRef>>, Cancelled> {
        let dependency_functions = self.dependencies.trait_functions(trait_ref)?;
        let local_functions = self.local_index.trait_functions_by_trait.get(&trait_ref);
        if local_functions.is_none() && dependency_functions.is_none() {
            return Ok(None);
        }

        let mut functions = local_functions.cloned().unwrap_or_default();
        self.dependencies
            .extend(&mut functions, dependency_functions.unwrap_or_default())?;
        Ok(Some(functions))
    }

    /// Returns same-name functions, preserving whether the trait was visible.
    #[rg_std::cancelable("lookup candidates")]
    pub fn trait_functions_by_name(
        &self,
        trait_ref: TraitDefRef,
        name: &str,
    ) -> Result<Option<UniqueVec<FunctionRef>>, Cancelled> {
        let dependency_functions = self.dependencies.trait_functions_by_name(trait_ref, name)?;
        let local_functions_by_name = self
            .local_index
            .trait_functions_by_trait_and_name
            .get(&trait_ref);
        if local_functions_by_name.is_none() && dependency_functions.is_none() {
            return Ok(None);
        }

        let mut functions = local_functions_by_name
            .and_then(|by_name| by_name.get(name))
            .cloned()
            .unwrap_or_default();
        self.dependencies
            .extend(&mut functions, dependency_functions.unwrap_or_default())?;
        Ok(Some(functions))
    }
}

// ==============================================================================
// Dependency-Only Lookup
// ==============================================================================

/// The dependency half of one use-site query.
///
/// `indexes` preserves DefMap visibility order. `results` is shared with other use-site queries
/// that have the same ordered dependency crate list. This type deliberately has no local index, so
/// its memoized candidates are safe to reuse across sibling targets.
#[derive(Debug, Clone)]
struct DependencyLookup<'item> {
    indexes: Arc<Vec<&'item ItemLookupIndex>>,
    results: Arc<Mutex<DependencyLookupResults>>,
    operation_cache: Arc<ItemLookupQueryCacheInner>,
    cancellation: CancellationToken,
}

impl rg_std::Cancelable for DependencyLookup<'_> {
    fn check_cancelled(&self, checkpoint: &'static str) -> Result<(), Cancelled> {
        rg_std::Cancelable::check_cancelled(&self.cancellation, checkpoint)
    }
}

impl DependencyLookup<'_> {
    /// Build each union privately. A cancelled scan must leave the shared cache without this key,
    /// so another use site can compute the complete answer later.
    fn extend<T: PartialEq>(
        &self,
        result: &mut UniqueVec<T>,
        candidates: impl IntoIterator<Item = T>,
    ) -> Result<(), Cancelled> {
        for (index, candidate) in candidates.into_iter().enumerate() {
            if index % 64 == 0 {
                #[cfg(test)]
                tests::candidate_checkpoint(&self.cancellation);
                rg_std::check_cancel!(self, "lookup candidates");
            }
            result.push(candidate);
        }
        rg_std::check_cancel!(self, "lookup candidates");
        Ok(())
    }

    #[rg_std::cancelable("lookup candidates")]
    fn inherent_functions_for_type_and_name(
        &self,
        ty: TypeDefRef,
        name: &str,
    ) -> Result<UniqueVec<FunctionRef>, Cancelled> {
        let key = (ty, Name::new(name));
        let mut results = self
            .results
            .lock()
            .expect("dependency lookup results lock should not be poisoned");
        if let Some(functions) = results.inherent_functions_by_type_and_name.get(&key) {
            self.operation_cache.record_dependency_result_hit();
            return Ok(functions.clone());
        }

        self.operation_cache.record_dependency_result_miss();
        let mut functions = UniqueVec::new();
        for index in self.indexes.iter() {
            rg_std::check_cancel!(self, "dependency candidate index");
            if let Some(indexed) = index
                .inherent_functions_by_type_and_name
                .get(&ty)
                .and_then(|by_name| by_name.get(name))
            {
                self.extend(&mut functions, indexed.iter().copied())?;
            }
        }
        results
            .inherent_functions_by_type_and_name
            .insert(key, functions.clone());
        Ok(functions)
    }

    #[rg_std::cancelable("lookup candidates")]
    fn structural_inherent_impls(&self) -> Result<UniqueVec<ImplRef>, Cancelled> {
        let mut results = self
            .results
            .lock()
            .expect("dependency lookup results lock should not be poisoned");
        if let Some(impls) = &results.structural_inherent_impls {
            self.operation_cache.record_dependency_result_hit();
            return Ok(impls.clone());
        }

        self.operation_cache.record_dependency_result_miss();
        let mut impls = UniqueVec::new();
        for index in self.indexes.iter() {
            rg_std::check_cancel!(self, "dependency candidate index");
            self.extend(&mut impls, index.structural_inherent_impls.iter().copied())?;
        }
        results.structural_inherent_impls = Some(impls.clone());
        Ok(impls)
    }

    #[rg_std::cancelable("lookup candidates")]
    fn inherent_impls_for_type(&self, ty: TypeDefRef) -> Result<UniqueVec<ImplRef>, Cancelled> {
        let mut results = self
            .results
            .lock()
            .expect("dependency lookup results lock should not be poisoned");
        if let Some(impls) = results.inherent_impls_by_type.get(&ty) {
            self.operation_cache.record_dependency_result_hit();
            return Ok(impls.clone());
        }

        self.operation_cache.record_dependency_result_miss();
        let mut impls = UniqueVec::new();
        for index in self.indexes.iter() {
            rg_std::check_cancel!(self, "dependency candidate index");
            if let Some(indexed) = index.inherent_impls_by_type.get(&ty) {
                self.extend(&mut impls, indexed.iter().copied())?;
            }
        }
        results.inherent_impls_by_type.insert(ty, impls.clone());
        Ok(impls)
    }

    #[rg_std::cancelable("lookup candidates")]
    fn trait_impls_for_type(&self, ty: TypeDefRef) -> Result<UniqueVec<TraitImplRef>, Cancelled> {
        let mut results = self
            .results
            .lock()
            .expect("dependency lookup results lock should not be poisoned");
        if let Some(impls) = results.trait_impls_by_type.get(&ty) {
            self.operation_cache.record_dependency_result_hit();
            return Ok(impls.clone());
        }

        self.operation_cache.record_dependency_result_miss();
        let mut impls = UniqueVec::new();
        for index in self.indexes.iter() {
            rg_std::check_cancel!(self, "dependency candidate index");
            if let Some(indexed) = index.trait_impls_by_type.get(&ty) {
                self.extend(
                    &mut impls,
                    indexed.iter().map(|trait_impl| trait_impl.expand()),
                )?;
            }
        }
        results.trait_impls_by_type.insert(ty, impls.clone());
        Ok(impls)
    }

    #[rg_std::cancelable("lookup candidates")]
    fn trait_impls_for_trait(
        &self,
        trait_ref: TraitDefRef,
    ) -> Result<Option<UniqueVec<TraitImplRef>>, Cancelled> {
        let mut results = self
            .results
            .lock()
            .expect("dependency lookup results lock should not be poisoned");
        if let Some(impls) = results.trait_impls_by_trait.get(&trait_ref) {
            self.operation_cache.record_dependency_result_hit();
            return Ok(impls.clone());
        }

        self.operation_cache.record_dependency_result_miss();
        let mut trait_was_indexed = false;
        let mut impls = UniqueVec::new();
        for index in self.indexes.iter() {
            rg_std::check_cancel!(self, "dependency candidate index");
            let Some(indexed) = index.trait_impls_by_trait.get(&trait_ref) else {
                continue;
            };
            trait_was_indexed = true;
            self.extend(
                &mut impls,
                indexed.all.iter().map(|impl_ref| TraitImplRef {
                    impl_ref: impl_ref.expand(),
                    trait_ref,
                }),
            )?;
        }
        let result = trait_was_indexed.then_some(impls);
        results
            .trait_impls_by_trait
            .insert(trait_ref, result.clone());
        Ok(result)
    }

    #[rg_std::cancelable("lookup candidates")]
    fn trait_impl_candidates_for_self_head(
        &self,
        trait_ref: TraitDefRef,
        self_head: Option<TraitImplSelfHead>,
    ) -> Result<Option<UniqueVec<TraitImplRef>>, Cancelled> {
        let key = (trait_ref, self_head);
        let mut results = self
            .results
            .lock()
            .expect("dependency lookup results lock should not be poisoned");
        if let Some(impls) = results.trait_impl_candidates_by_self_head.get(&key) {
            self.operation_cache.record_dependency_result_hit();
            return Ok(impls.clone());
        }

        self.operation_cache.record_dependency_result_miss();
        let mut trait_was_indexed = false;
        let mut impls = UniqueVec::new();
        for index in self.indexes.iter() {
            rg_std::check_cancel!(self, "dependency candidate index");
            let Some(indexed) = index.trait_impl_candidates_for_self_head(trait_ref, self_head)
            else {
                continue;
            };
            trait_was_indexed = true;
            self.extend(
                &mut impls,
                indexed.iter().map(|impl_ref| TraitImplRef {
                    impl_ref: impl_ref.expand(),
                    trait_ref,
                }),
            )?;
        }
        let result = trait_was_indexed.then_some(impls);
        results
            .trait_impl_candidates_by_self_head
            .insert(key, result.clone());
        Ok(result)
    }

    #[rg_std::cancelable("lookup candidates")]
    fn traits_with_functions(&self) -> Result<UniqueVec<TraitDefRef>, Cancelled> {
        let mut results = self
            .results
            .lock()
            .expect("dependency lookup results lock should not be poisoned");
        if let Some(traits) = &results.traits_with_functions {
            self.operation_cache.record_dependency_result_hit();
            return Ok(traits.clone());
        }

        self.operation_cache.record_dependency_result_miss();
        let mut traits = UniqueVec::new();
        for index in self.indexes.iter() {
            rg_std::check_cancel!(self, "dependency candidate index");
            self.extend(&mut traits, index.traits_with_functions.iter().copied())?;
        }
        results.traits_with_functions = Some(traits.clone());
        Ok(traits)
    }

    #[rg_std::cancelable("lookup candidates")]
    fn traits_with_associated_items(&self) -> Result<UniqueVec<TraitDefRef>, Cancelled> {
        let mut results = self
            .results
            .lock()
            .expect("dependency lookup results lock should not be poisoned");
        if let Some(traits) = &results.traits_with_associated_items {
            self.operation_cache.record_dependency_result_hit();
            return Ok(traits.clone());
        }

        self.operation_cache.record_dependency_result_miss();
        let mut traits = UniqueVec::new();
        for index in self.indexes.iter() {
            rg_std::check_cancel!(self, "dependency candidate index");
            self.extend(
                &mut traits,
                index.traits_with_associated_items.iter().copied(),
            )?;
        }
        results.traits_with_associated_items = Some(traits.clone());
        Ok(traits)
    }

    #[rg_std::cancelable("lookup candidates")]
    fn traits_with_item_name(
        &self,
        kind: TraitItemKind,
        name: &str,
    ) -> Result<UniqueVec<TraitDefRef>, Cancelled> {
        let key = (kind, Name::new(name));
        let mut results = self
            .results
            .lock()
            .expect("dependency lookup results lock should not be poisoned");
        if let Some(traits) = results.traits_by_item_name.get(&key) {
            self.operation_cache.record_dependency_result_hit();
            return Ok(traits.clone());
        }

        self.operation_cache.record_dependency_result_miss();
        let mut traits = UniqueVec::new();
        for index in self.indexes.iter() {
            rg_std::check_cancel!(self, "dependency candidate index");
            if let Some(indexed) = index.traits_by_item_name.get(&key.1) {
                self.extend(&mut traits, kind.declaring_traits(indexed).iter().copied())?;
            }
        }
        results.traits_by_item_name.insert(key, traits.clone());
        Ok(traits)
    }

    #[rg_std::cancelable("lookup candidates")]
    fn trait_functions(
        &self,
        trait_ref: TraitDefRef,
    ) -> Result<Option<UniqueVec<FunctionRef>>, Cancelled> {
        let mut results = self
            .results
            .lock()
            .expect("dependency lookup results lock should not be poisoned");
        if let Some(functions) = results.trait_functions_by_trait.get(&trait_ref) {
            self.operation_cache.record_dependency_result_hit();
            return Ok(functions.clone());
        }

        self.operation_cache.record_dependency_result_miss();
        let mut trait_was_indexed = false;
        let mut functions = UniqueVec::new();
        for index in self.indexes.iter() {
            rg_std::check_cancel!(self, "dependency candidate index");
            let Some(indexed) = index.trait_functions_by_trait.get(&trait_ref) else {
                continue;
            };
            trait_was_indexed = true;
            self.extend(&mut functions, indexed.iter().copied())?;
        }
        let result = trait_was_indexed.then_some(functions);
        results
            .trait_functions_by_trait
            .insert(trait_ref, result.clone());
        Ok(result)
    }

    #[rg_std::cancelable("lookup candidates")]
    fn trait_functions_by_name(
        &self,
        trait_ref: TraitDefRef,
        name: &str,
    ) -> Result<Option<UniqueVec<FunctionRef>>, Cancelled> {
        let key = (trait_ref, Name::new(name));
        let mut results = self
            .results
            .lock()
            .expect("dependency lookup results lock should not be poisoned");
        if let Some(functions) = results.trait_functions_by_trait_and_name.get(&key) {
            self.operation_cache.record_dependency_result_hit();
            return Ok(functions.clone());
        }

        self.operation_cache.record_dependency_result_miss();
        let mut trait_was_indexed = false;
        let mut functions = UniqueVec::new();
        for index in self.indexes.iter() {
            rg_std::check_cancel!(self, "dependency candidate index");
            let Some(indexed_by_name) = index.trait_functions_by_trait_and_name.get(&trait_ref)
            else {
                continue;
            };
            trait_was_indexed = true;
            if let Some(indexed) = indexed_by_name.get(name) {
                self.extend(&mut functions, indexed.iter().copied())?;
            }
        }
        let result = trait_was_indexed.then_some(functions);
        results
            .trait_functions_by_trait_and_name
            .insert(key, result.clone());
        Ok(result)
    }
}

// ==============================================================================
// Operation-Owned Cache
// ==============================================================================

/// Structural cache statistics captured for one build or analysis operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ItemLookupQueryCacheStats {
    /// Distinct ordered dependency sets that allocated result storage.
    pub dependency_cache_constructions: usize,
    /// Use-site queries that reused result storage for an existing dependency set.
    pub dependency_cache_reuses: usize,
    /// Dependency lookup keys served from previously computed results.
    pub dependency_result_hits: usize,
    /// Dependency lookup keys computed from the persisted indexes for the first time.
    pub dependency_result_misses: usize,
}

/// Shared dependency-result storage for one build or frozen analysis operation.
///
/// Create one cache around the family of [`ItemLookupQuery`] values built by that operation. The
/// ordered dependency crate list selects one `DependencyLookupResults` allocation, while each
/// query continues to borrow its own local and dependency indexes. This cache owns only compact
/// candidate identities and does not become part of persisted Semantic IR.
#[derive(Debug, Clone, Default)]
pub struct ItemLookupQueryCache {
    inner: Arc<ItemLookupQueryCacheInner>,
}

impl ItemLookupQueryCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stats(&self) -> ItemLookupQueryCacheStats {
        self.inner.stats()
    }

    fn dependencies<'item>(
        &self,
        dependency_crates: Vec<CrateRef>,
        indexes: Vec<&'item ItemLookupIndex>,
        cancellation: CancellationToken,
    ) -> DependencyLookup<'item> {
        DependencyLookup {
            indexes: Arc::new(indexes),
            results: self.inner.results_for(dependency_crates),
            operation_cache: Arc::clone(&self.inner),
            cancellation,
        }
    }
}

/// Shared ownership retained by the operation cache and every query built from it.
#[derive(Debug, Default)]
struct ItemLookupQueryCacheInner {
    dependency_results: Mutex<HashMap<Vec<CrateRef>, Arc<Mutex<DependencyLookupResults>>>>,
    dependency_cache_constructions: AtomicUsize,
    dependency_cache_reuses: AtomicUsize,
    dependency_result_hits: AtomicUsize,
    dependency_result_misses: AtomicUsize,
}

impl ItemLookupQueryCacheInner {
    fn results_for(&self, dependency_crates: Vec<CrateRef>) -> Arc<Mutex<DependencyLookupResults>> {
        let mut results = self
            .dependency_results
            .lock()
            .expect("dependency lookup cache pool lock should not be poisoned");
        match results.entry(dependency_crates) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                self.dependency_cache_reuses.fetch_add(1, Ordering::Relaxed);
                Arc::clone(entry.get())
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                self.dependency_cache_constructions
                    .fetch_add(1, Ordering::Relaxed);
                Arc::clone(entry.insert(Arc::default()))
            }
        }
    }

    fn record_dependency_result_hit(&self) {
        self.dependency_result_hits.fetch_add(1, Ordering::Relaxed);
    }

    fn record_dependency_result_miss(&self) {
        self.dependency_result_misses
            .fetch_add(1, Ordering::Relaxed);
    }

    fn stats(&self) -> ItemLookupQueryCacheStats {
        ItemLookupQueryCacheStats {
            dependency_cache_constructions: self
                .dependency_cache_constructions
                .load(Ordering::Relaxed),
            dependency_cache_reuses: self.dependency_cache_reuses.load(Ordering::Relaxed),
            dependency_result_hits: self.dependency_result_hits.load(Ordering::Relaxed),
            dependency_result_misses: self.dependency_result_misses.load(Ordering::Relaxed),
        }
    }
}

/// Lazily computed unions from the indexes in one ordered dependency set.
///
/// These maps never contain candidates from the use-site crate. A missing map entry means that the
/// lookup key has not been requested yet. Trait-keyed maps need one more state after computation:
///
/// - `None` means no dependency index contained the trait;
/// - `Some(empty)` means the trait was visible but the narrower lookup found no candidate;
/// - `Some(non-empty)` contains the dependency candidates.
///
/// For example, `(DisplayLike, Primitive(UnsignedInt(U32)))` caches the union of direct `u32`
/// impls and conservative blanket impls from dependencies. It does not cache the later type proof,
/// which may depend on inference state owned by the caller.
#[derive(Debug, Default)]
struct DependencyLookupResults {
    inherent_impls_by_type: HashMap<TypeDefRef, UniqueVec<ImplRef>>,
    inherent_functions_by_type_and_name: HashMap<(TypeDefRef, Name), UniqueVec<FunctionRef>>,
    structural_inherent_impls: Option<UniqueVec<ImplRef>>,
    trait_impls_by_type: HashMap<TypeDefRef, UniqueVec<TraitImplRef>>,
    trait_impls_by_trait: HashMap<TraitDefRef, Option<UniqueVec<TraitImplRef>>>,
    trait_impl_candidates_by_self_head:
        HashMap<(TraitDefRef, Option<TraitImplSelfHead>), Option<UniqueVec<TraitImplRef>>>,
    traits_with_functions: Option<UniqueVec<TraitDefRef>>,
    traits_with_associated_items: Option<UniqueVec<TraitDefRef>>,
    traits_by_item_name: HashMap<(TraitItemKind, Name), UniqueVec<TraitDefRef>>,
    trait_functions_by_trait: HashMap<TraitDefRef, Option<UniqueVec<FunctionRef>>>,
    trait_functions_by_trait_and_name: HashMap<(TraitDefRef, Name), Option<UniqueVec<FunctionRef>>>,
}

#[cfg(test)]
mod tests {
    use super::{CancellationToken, CrateRef, ItemLookupIndex, ItemLookupQueryCache, TraitDefRef};
    use rg_ir_model::{CrateId, DefMapRef, PackageSlot, TraitId};

    thread_local! {
        static CANCEL_AFTER_BATCHES: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    }

    pub(super) fn candidate_checkpoint(cancellation: &CancellationToken) {
        CANCEL_AFTER_BATCHES.with(|remaining| {
            if let Some(count) = remaining.get() {
                if count == 0 {
                    remaining.set(None);
                    cancellation.cancel();
                } else {
                    remaining.set(Some(count - 1));
                }
            }
        });
    }

    #[test]
    fn cancelled_candidate_union_does_not_enter_the_shared_cache() {
        let target = CrateRef {
            package: PackageSlot(0),
            crate_id: CrateId(0),
        };
        let mut index = ItemLookupIndex::default();
        index.traits_with_functions = (0..130)
            .map(|id| TraitDefRef {
                origin: DefMapRef::Crate(target),
                id: TraitId(id),
            })
            .collect();
        let cache = ItemLookupQueryCache::new();
        let interrupted = cache.dependencies(vec![target], vec![&index], CancellationToken::new());
        CANCEL_AFTER_BATCHES.with(|remaining| remaining.set(Some(1)));
        interrupted
            .traits_with_functions()
            .expect_err("second candidate batch cancels collection");
        assert!(CANCEL_AFTER_BATCHES.with(|remaining| remaining.get().is_none()));
        let retry = cache.dependencies(vec![target], vec![&index], CancellationToken::new());
        assert_eq!(
            retry
                .traits_with_functions()
                .expect("retry completes")
                .len(),
            130
        );
        assert_eq!(cache.stats().dependency_result_hits, 0);
        assert_eq!(
            retry
                .traits_with_functions()
                .expect("complete result is reusable")
                .len(),
            130
        );
        assert_eq!(cache.stats().dependency_result_hits, 1);
    }
}
