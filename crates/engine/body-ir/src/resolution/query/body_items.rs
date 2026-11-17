//! A small declaration index for items supplied by the active body view.
//!
//! Crate indexes contain module-level declarations. Body resolution also needs declarations nested
//! within the body or its enclosing bodies:
//!
//! ```text
//! fn inspect() {
//!     struct Local;
//!     impl Local { fn run(&self) {} }
//!     Local.run();
//! }
//! ```
//!
//! Those declarations belong to body stores rather than the crate index. The first body query scans
//! the few body item stores that can affect this body and builds a compact lookup index. Later
//! queries read the same index through [`BodyLocalItemCache`](crate::resolution::cache::BodyLocalItemCache).

use std::collections::HashMap;

use rg_def_map::DefMapSource;
use rg_ir_model::{
    AssocItemId, DefMapRef, ImplRef, SemanticItemRef, TraitDefRef, TraitImplRef, TypeDefRef,
};
use rg_semantic_ir::{SemanticItemSource, SemanticItemView};
use rg_source::ReadError;
use rg_std::UniqueVec;

use crate::resolution::{BodyResolutionContext, cache::BodyLocalItemIndex};

/// Read adapter for declarations nested within this outer body.
pub(crate) struct BodyLocalItemQuery<'context, 'query, D, I> {
    context: &'context BodyResolutionContext<'query, D, I>,
}

impl<'context, 'query, D, I> BodyLocalItemQuery<'context, 'query, D, I>
where
    D: DefMapSource + Copy,
    I: SemanticItemSource<'query> + Copy,
{
    pub(crate) fn new(context: &'context BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Return body-local inherent impls whose `Self` resolves to this type.
    pub(super) fn inherent_impls_for_type(&self, ty: TypeDefRef) -> Result<&[ImplRef], ReadError> {
        Ok(self
            .index()?
            .inherent_impls_by_type
            .get(&ty)
            .map(UniqueVec::as_slice)
            .unwrap_or_default())
    }

    /// Return body-local trait impls whose `Self` resolves to this type.
    pub(super) fn trait_impls_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<&[TraitImplRef], ReadError> {
        Ok(self
            .index()?
            .trait_impls_by_type
            .get(&ty)
            .map(UniqueVec::as_slice)
            .unwrap_or_default())
    }

    /// Return body-local impls for already-selected traits in one store pass.
    pub(crate) fn trait_impls_for_traits<'a>(
        &'a self,
        trait_refs: &'a [TraitDefRef],
    ) -> Result<impl Iterator<Item = TraitImplRef> + 'a, ReadError> {
        // Solver queries usually ask for one trait and encounter no local impls. Borrow the
        // requested names; an empty index performs no membership work or allocation.
        Ok(self
            .index()?
            .trait_impls
            .iter()
            .filter(move |candidate| trait_refs.contains(&candidate.trait_ref))
            .copied())
    }

    /// Return body-local traits declaring a function with this name.
    pub(super) fn traits_with_function_name(
        &self,
        name: &str,
    ) -> Result<&[TraitDefRef], ReadError> {
        Ok(self
            .index()?
            .traits_by_function_name
            .get(name)
            .map(UniqueVec::as_slice)
            .unwrap_or_default())
    }

    /// Return body-local traits declaring an associated const with this name.
    pub(super) fn traits_with_const_name(&self, name: &str) -> Result<&[TraitDefRef], ReadError> {
        Ok(self
            .index()?
            .traits_by_const_name
            .get(name)
            .map(UniqueVec::as_slice)
            .unwrap_or_default())
    }

    /// Return body-local traits that declare at least one function.
    pub(super) fn traits_with_functions(&self) -> Result<&[TraitDefRef], ReadError> {
        Ok(self.index()?.traits_with_functions.as_slice())
    }

    /// Return body-local traits that declare at least one associated item.
    pub(super) fn traits_with_associated_items(&self) -> Result<&[TraitDefRef], ReadError> {
        Ok(self.index()?.traits_with_associated_items.as_slice())
    }

    /// Build the local declaration index once for queries throughout body inference.
    fn build_index(&self) -> Result<BodyLocalItemIndex, ReadError> {
        let mut index = BodyLocalItemIndex::default();
        for items in self.body_lookup_items()? {
            // Follow member lists using this inventory, so each body's data is acquired once.
            let item_names: HashMap<_, _> = items
                .iter()
                .filter_map(|item| Some((item.item(), item.name()?)))
                .collect();

            // Group local impls by receiver so lookup can combine them with crate-level impls.
            for view in &items {
                let SemanticItemRef::Impl(impl_ref) = view.item() else {
                    continue;
                };
                let Some(impl_data) = view.impl_data() else {
                    continue;
                };
                if let Some(trait_ref) = impl_data.resolved_trait_ref.as_option() {
                    let trait_impl = TraitImplRef {
                        impl_ref,
                        trait_ref: *trait_ref,
                    };
                    index.trait_impls.push(trait_impl);
                    if let Some(self_ty) = impl_data.resolved_self_ty.as_option() {
                        index
                            .trait_impls_by_type
                            .entry(*self_ty)
                            .or_default()
                            .push(trait_impl);
                    }
                    continue;
                }

                if impl_data.trait_ref.is_some() {
                    continue;
                }
                let Some(self_ty) = impl_data.resolved_self_ty.as_option() else {
                    continue;
                };
                index
                    .inherent_impls_by_type
                    .entry(*self_ty)
                    .or_default()
                    .push(impl_ref);
            }

            // Trait declarations provide the name-first surfaces used before impl matching. A
            // method lookup for `value.draw()` discovers `Paint` here; receiver proof happens in
            // `BodyImplQuery` after lexical scope has filtered the declaration surface.
            for view in &items {
                let SemanticItemRef::Trait(trait_ref) = view.item() else {
                    continue;
                };
                let members = self
                    .context
                    .item_query()
                    .associated_items(trait_ref.member_store())?;
                if !members.is_empty() {
                    index.traits_with_associated_items.push(trait_ref);
                }

                let mut has_function = false;
                for item in members {
                    has_function |= matches!(item, AssocItemId::Function(_));
                    let Some(name) = item_names.get(&item.semantic_ref(trait_ref.member_store()))
                    else {
                        continue;
                    };
                    match item {
                        AssocItemId::Function(_) => {
                            index
                                .traits_by_function_name
                                .entry((*name).clone())
                                .or_default()
                                .push(trait_ref);
                        }
                        AssocItemId::Const(_) => {
                            index
                                .traits_by_const_name
                                .entry((*name).clone())
                                .or_default()
                                .push(trait_ref);
                        }
                        AssocItemId::TypeAlias(_) => {}
                    }
                }
                if has_function {
                    index.traits_with_functions.push(trait_ref);
                }
            }
        }
        Ok(index)
    }

    fn index(&self) -> Result<&BodyLocalItemIndex, ReadError> {
        self.context
            .body_local_item_cache()
            .index_or_try_init(|| self.build_index())
    }

    /// Gather body declarations that can affect the current body lookup.
    fn body_lookup_items(&self) -> Result<Vec<Vec<SemanticItemView<'query>>>, ReadError> {
        let mut origins = UniqueVec::new();

        // Check the active body first, then the body-local modules that own this declaration and
        // its fallback. Target declarations are handled by the crate lookup index.
        origins.push(DefMapRef::Body(self.context.body_ref()));
        for module in [
            self.context.body().owner_module(),
            self.context.body().fallback_module(),
        ] {
            if let DefMapRef::Body(_) = module.origin {
                origins.push(module.origin);
            }
        }

        let item_query = self.context.item_query();
        let mut groups = Vec::new();
        for origin in origins {
            groups.push(item_query.semantic_items_for_origin(origin)?);
        }
        Ok(groups)
    }
}
