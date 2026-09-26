//! Discover applicable source impls and the items they expose for a receiver.
//!
//! The declaration index narrows candidates, then the shared solver queries match their headers
//! and prove their predicates. Source-type lowering stays in `lowering`.

mod candidates;
mod receiver;
mod trait_impl;

use rg_def_map::DefMapSource;
use rg_ir_model::ImplRef;
use rg_semantic_ir::ItemStoreSource;

pub(crate) use self::candidates::TraitImplFilter;
pub use self::receiver::{InherentImplMatch, ReceiverFunctionCandidate, ReceiverImplMatches};
use crate::{
    Substitution, Ty, TyContext, lookup::ItemPathQuery, lowering::TypeLoweringQuery,
    signature::ImplHeader, solver::SolverScope, trait_selection::TraitSelectionQuery,
};

/// Receiver-based impl discovery and selection in one type-query context.
/// For a receiver such as `Vec<u8>`, the index finds plausible impl blocks, then solver queries
/// decide whether their headers and bounds apply. The results retain source identities so member
/// and implementation lookups can return the declarations those impls expose.
pub struct ImplQuery<'query, D, I, R = ItemPathQuery<'query, D, I>> {
    context: TyContext<'query, D, I>,
    resolver: R,
}

impl<'query, D, I> ImplQuery<'query, D, I, ItemPathQuery<'query, D, I>>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
{
    pub fn new(context: TyContext<'query, D, I>) -> Self {
        let resolver = context.item_paths().clone();
        Self { context, resolver }
    }
}

impl<'query, D, I, R> ImplQuery<'query, D, I, R>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
    R: SolverScope<Error = D::Error>,
{
    pub fn with_resolver(context: TyContext<'query, D, I>, resolver: R) -> Self {
        Self { context, resolver }
    }

    /// Lower the impl's receiver and positional trait arguments without its requirements.
    pub fn impl_header(&self, impl_ref: ImplRef) -> Result<Option<ImplHeader>, D::Error> {
        let lowering = TypeLoweringQuery::new(self.context.item_paths(), &self.resolver);
        lowering.with_storage(|cx| {
            Ok(lowering
                .impl_header(cx, impl_ref)?
                .map(|header| header.raise(cx)))
        })
    }

    /// Discover bindings from the impl's receiver pattern. This does not check applicability;
    /// associated-alias lookup needs these bindings before predicates can be lowered.
    pub fn impl_self_substitution(
        &self,
        impl_ref: ImplRef,
        receiver_ty: &Ty,
    ) -> Result<Option<Substitution>, D::Error> {
        TraitSelectionQuery::with_resolver(self.context.clone(), &self.resolver)
            .match_impl_header(impl_ref, receiver_ty)
    }
}
