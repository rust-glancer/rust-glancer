//! Implementation lookup over semantic-shaped item stores.
//!
//! Goto-implementation needs type/impl reasoning, but not source spans or editor labels. This
//! query keeps the reusable search at the ref level so view code can project results into the
//! declaration shape that UI-facing analysis expects.

use rg_ir_model::{AssocItemId, FunctionRef, ImplRef, ItemOwner, TraitRef, TypeDefRef};
use rg_ir_storage::{DefMapSource, ItemLookupIndex, ItemStoreSource, TargetItemQuery};
use rg_std::UniqueVec;

use crate::{Autoderef, AutoderefMode, ImplMatcher, ItemPathQuery, ReferencePeelingCandidates, Ty};

/// Ref-level implementation lookup shared by view and analysis adapters.
pub struct ImplementationQuery<'query, D, I> {
    item_paths: ItemPathQuery<'query, D, I>,
    target_items: TargetItemQuery<'query, D, I>,
    lookup_index: &'query ItemLookupIndex,
}

impl<'query, D, I> ImplementationQuery<'query, D, I>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
{
    /// Creates an implementation query over a target-scoped receiver lookup index.
    pub fn with_index(
        item_paths: ItemPathQuery<'query, D, I>,
        target_items: TargetItemQuery<'query, D, I>,
        lookup_index: &'query ItemLookupIndex,
    ) -> Self {
        Self {
            item_paths,
            target_items,
            lookup_index,
        }
    }

    /// Returns impl blocks for all nominal type definitions reachable through reference peeling.
    pub fn impls_for_ty(&self, ty: &Ty) -> Result<UniqueVec<ImplRef>, D::Error> {
        let mut impls = UniqueVec::new();
        for candidate in ReferencePeelingCandidates::new(ty) {
            for ty in candidate.ty().as_nominals() {
                for impl_ref in self.impls_for_type_def(ty.def)? {
                    impls.push(impl_ref);
                }
            }
        }
        Ok(impls)
    }

    /// Returns impl blocks whose resolved self type mentions this nominal type definition.
    pub fn impls_for_type_def(&self, ty: TypeDefRef) -> Result<UniqueVec<ImplRef>, D::Error> {
        Ok(self.lookup_index.impls_for_type(ty))
    }

    /// Returns impl blocks that resolve to the requested trait.
    pub fn impls_for_trait(&self, trait_ref: TraitRef) -> Result<UniqueVec<ImplRef>, D::Error> {
        Ok(self.lookup_index.impls_for_trait(trait_ref))
    }

    /// Returns concrete functions that implement or correspond to the selected function.
    ///
    /// Trait methods expand to matching impl methods. Impl methods are already concrete
    /// implementations and are returned as-is. Free functions do not have implementations.
    pub fn function_implementations(
        &self,
        function: FunctionRef,
        receiver_ty: Option<&Ty>,
    ) -> Result<UniqueVec<FunctionRef>, D::Error> {
        let Some(data) = self.item_paths.items().function_data(function)? else {
            return Ok(UniqueVec::new());
        };

        match data.owner {
            ItemOwner::Trait(trait_id) => self.impl_methods_for_trait_method(
                TraitRef {
                    origin: function.origin,
                    id: trait_id,
                },
                data.name.as_str(),
                receiver_ty,
            ),
            ItemOwner::Impl(_) => Ok([function].into_iter().collect()),
            ItemOwner::Module(_) => Ok(UniqueVec::new()),
        }
    }

    /// Returns impl methods matching a trait method, optionally narrowed to one receiver type.
    pub fn impl_methods_for_trait_method(
        &self,
        trait_ref: TraitRef,
        method_name: &str,
        receiver_ty: Option<&Ty>,
    ) -> Result<UniqueVec<FunctionRef>, D::Error> {
        match receiver_ty {
            Some(receiver_ty) => {
                self.impl_methods_for_trait_method_receiver(trait_ref, method_name, receiver_ty)
            }
            None => self.impl_methods_for_trait_method_any_receiver(trait_ref, method_name),
        }
    }

    fn impl_methods_for_trait_method_receiver(
        &self,
        trait_ref: TraitRef,
        method_name: &str,
        receiver_ty: &Ty,
    ) -> Result<UniqueVec<FunctionRef>, D::Error> {
        let autoderef = Autoderef::with_index(
            self.item_paths.clone(),
            self.target_items.clone(),
            self.lookup_index,
        );
        let matcher = ImplMatcher::new(self.item_paths.clone(), self.target_items.clone());
        let mut functions = UniqueVec::new();

        for candidate in autoderef.candidates(AutoderefMode::MethodReceiver, receiver_ty) {
            let candidate = candidate?;
            for ty in candidate.ty().as_nominals() {
                let trait_impls = self
                    .lookup_index
                    .trait_impls_for_type(ty.def)
                    .cloned()
                    .unwrap_or_default();
                for trait_impl in trait_impls {
                    if trait_impl.trait_ref != trait_ref {
                        continue;
                    }
                    // The nominal type match can still include generic impls for other concrete
                    // args. Reuse method lookup's applicability check so implementation lookup
                    // follows the receiver the user actually called the method on.
                    if !matcher
                        .trait_impl_applicability(trait_impl, ty)?
                        .is_applicable()
                    {
                        continue;
                    }
                    for function in self.matching_impl_methods(trait_impl.impl_ref, method_name)? {
                        functions.push(function);
                    }
                }
            }
        }

        Ok(functions)
    }

    fn impl_methods_for_trait_method_any_receiver(
        &self,
        trait_ref: TraitRef,
        method_name: &str,
    ) -> Result<UniqueVec<FunctionRef>, D::Error> {
        let mut functions = UniqueVec::new();
        for impl_ref in self.impls_for_trait(trait_ref)? {
            for function in self.matching_impl_methods(impl_ref, method_name)? {
                functions.push(function);
            }
        }
        Ok(functions)
    }

    fn matching_impl_methods(
        &self,
        impl_ref: ImplRef,
        method_name: &str,
    ) -> Result<UniqueVec<FunctionRef>, D::Error> {
        let Some(data) = self.item_paths.items().impl_data(impl_ref)? else {
            return Ok(UniqueVec::new());
        };

        let mut functions = UniqueVec::new();
        for item in &data.items {
            let &AssocItemId::Function(id) = item else {
                continue;
            };
            let function = FunctionRef {
                origin: impl_ref.origin,
                id,
            };
            let Some(function_data) = self.item_paths.items().function_data(function)? else {
                continue;
            };
            if function_data.name.as_str() != method_name {
                continue;
            }
            functions.push(function);
        }
        Ok(functions)
    }
}
