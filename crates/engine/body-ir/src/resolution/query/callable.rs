//! Callable selected through a method receiver or associated-item prefix.

use rg_def_map::DefMapSource;
use rg_ir_model::FunctionRef;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::{ReceiverFunctionCandidate, Substitution, TraitSelection, Ty};

use crate::resolution::BodyResolutionContext;

/// One callable with all receiver and impl evidence needed to instantiate its signature.
///
/// Dot methods and static associated functions differ only in whether their declaration has a
/// `self` parameter. Once that syntax check is made, both must apply receiver bindings, selected
/// trait arguments, and explicit qualification in the same order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyCallableCandidate {
    function: FunctionRef,
    receiver_ty: Ty,
    subst: Substitution,
    trait_selection: Option<TraitSelection>,
}

impl BodyCallableCandidate {
    /// Instantiate a callable from the impl evidence that exposed its declaration.
    pub(crate) fn from_receiver_function<'query, D, I>(
        context: &BodyResolutionContext<'query, D, I>,
        receiver_ty: &Ty,
        candidate: ReceiverFunctionCandidate,
        extra_subst: Option<&Substitution>,
    ) -> Result<Option<Self>, PackageStoreError>
    where
        D: DefMapSource<Error = PackageStoreError> + Copy,
        I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
    {
        let function = candidate.function();
        let Some(function_data) = context.item_query().function_data(function)? else {
            return Ok(None);
        };

        let mut subst = context.generics().subst_for_selected_item_owner(
            function.origin,
            function_data.owner,
            receiver_ty,
            candidate
                .inherent_match()
                .map(|impl_match| impl_match.subst()),
        )?;
        let trait_selection = candidate.trait_selection().cloned();
        if let Some(selection) = trait_selection.as_ref() {
            // Keep impl-owned bindings as well as trait-owned arguments. Trait declarations mainly
            // consume the latter, while this also stays correct for fail-soft impl-owned items.
            subst.extend(selection.subst.as_substitution().clone());
            subst.extend(
                context
                    .generics()
                    .subst_for_trait_application(selection.application())?,
            );
        }
        // Written qualification is strongest at this call site. Applying it last makes
        // `<Widget as Factory<u16>>::make` retain `u16` even if selection kept an inference hole.
        if let Some(extra_subst) = extra_subst {
            subst.extend(extra_subst.clone());
        }

        Ok(Some(Self {
            function,
            receiver_ty: receiver_ty.clone(),
            subst,
            trait_selection,
        }))
    }

    pub(crate) fn function(&self) -> FunctionRef {
        self.function
    }

    pub(crate) fn receiver_ty(&self) -> &Ty {
        &self.receiver_ty
    }

    pub(crate) fn subst(&self) -> &Substitution {
        &self.subst
    }

    /// Return the trait proof to commit if call inference uniquely selects this candidate.
    pub(crate) fn trait_selection(&self) -> Option<&TraitSelection> {
        self.trait_selection.as_ref()
    }
}
