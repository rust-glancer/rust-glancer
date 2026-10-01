//! Implementation lookup over semantic-shaped item stores.
//!
//! Navigation needs type/impl reasoning, but not source spans or editor labels. This query keeps
//! the reusable search at the ref level so view code can project results into the declaration
//! shape that UI-facing analysis expects.

use rg_def_map::DefMapSource;
use rg_ir_model::{FunctionRef, GenericDefRef, ImplRef, ItemOwner, TraitDefRef, TypeDefRef};
use rg_semantic_ir::ItemStoreSource;
use rg_std::{ExpectedUnique, OperationError, UniqueVec};

use super::TraitImplFilter;
use crate::{
    ConstValue, GenericArg, GenericArgs, Substitution, Ty, TyContext,
    solver::{Outcome, SemanticDeclarations, SolverScope, TraitApplication},
};

/// Ref-level implementation lookup shared by view and analysis adapters.
pub struct ImplementationQuery<'query, D, I> {
    context: TyContext<'query, D, I>,
}

impl<'query, D, I> ImplementationQuery<'query, D, I>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
{
    /// Creates implementation lookup in one crate-scoped type-query environment.
    pub fn new(context: TyContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Find the method in the impl that matches a resolved trait call.
    ///
    /// A call can name `Convert::convert` with `Self = Source` and `T = u32`. Those recorded
    /// arguments let us look for `impl Convert<u32> for Source` and then its `convert` method.
    /// Return a method only when one impl is proven to apply, so navigation can keep the trait
    /// declaration when the available facts do not establish a destination.
    pub fn selected_call_implementation(
        &self,
        function: FunctionRef,
        args: &GenericArgs,
        scope: &impl SolverScope<Error = D::Error>,
    ) -> Result<Option<FunctionRef>, OperationError<D::Error>> {
        rg_std::check_cancel!(self.context, "selected call implementation");
        let paths = self.context.item_paths();
        let Some(data) = paths
            .items()
            .function_data(function)
            .map_err(OperationError::Source)?
        else {
            return Ok(None);
        };
        let ItemOwner::Trait(id) = data.owner else {
            return Ok(None);
        };
        let trait_ref = TraitDefRef {
            origin: function.origin,
            id,
        };

        // A call stores the trait's arguments followed by the method's own arguments.
        // For `Convert<u32>::convert::<u8>`, impl matching needs Self and u32, not the method's
        // u8. Bind arguments to their parameter identities, then take just the trait's slots.
        let generics = paths.generics();
        let function_params = generics
            .generics(GenericDefRef::Function(function))
            .map_err(OperationError::Source)?;
        let trait_params = generics
            .generics(GenericDefRef::Trait(trait_ref))
            .map_err(OperationError::Source)?;
        let args = Substitution::from_args(&function_params, args).args_for(&trait_params);
        let Some(receiver) = args.first().and_then(GenericArg::as_ty) else {
            return Ok(None);
        };
        // An unknown argument does not identify an impl. Keep the trait declaration instead of
        // guessing the missing arguments from whichever impls happen to be available.
        if args
            .iter()
            .any(|arg| arg.has_unknown() || matches!(arg, GenericArg::Const(ConstValue::Unknown)))
        {
            return Ok(None);
        }

        // Start with saved impls whose receiver shape could match, then include body-local
        // impls. This only narrows the search: `Wrapper<User>` and `Wrapper<Account>` have the same
        // outer shape, so their generic arguments and bounds still need to be checked.
        let Some(mut candidates) =
            TraitImplFilter::from(receiver).candidates(&self.context, trait_ref)
        else {
            rg_std::check_cancel!(self.context, "selected call implementation");
            return Ok(None);
        };
        candidates.extend(
            scope
                .local_trait_impls(trait_ref)
                .map_err(OperationError::Source)?,
        );

        // Check the complete trait arguments and each impl's bounds in the calling body's
        // scope. Self is the receiver type already chosen for this call, including any
        // dereferencing. Trying more receiver adjustments here could choose another method.
        let declarations = SemanticDeclarations::new(&self.context, scope);
        let implementation = declarations
            .with_table(|table, params| {
                let cx = table.interner();
                let callbacks = cx.track_callbacks();
                let application = TraitApplication {
                    def: trait_ref,
                    args: cx.lower_args(&args, params),
                };
                if callbacks.failure().is_some() {
                    return None;
                }
                match table.select_trait_impl(
                    application,
                    &[],
                    candidates.into_iter().map(|candidate| candidate.impl_ref),
                ) {
                    ExpectedUnique::One(selected) if selected.outcome == Outcome::Proven => {
                        Some(selected.impl_ref)
                    }
                    _ => None,
                }
            })
            .map_err(OperationError::Source)?;
        rg_std::check_cancel!(self.context, "selected call implementation");
        let Some(implementation) = implementation else {
            return Ok(None);
        };

        // An impl can inherit the trait's default body. It has no method declaration to visit,
        // so let navigation keep the trait method as its destination in that case.
        let methods = self.matching_impl_methods(implementation, data.name.as_str())?;
        Ok(match methods.as_slice() {
            [method] => Some(*method),
            _ => None,
        })
    }

    /// Returns impl blocks for all nominal type definitions reachable through reference peeling.
    pub fn impls_for_ty(&self, ty: &Ty) -> Result<UniqueVec<ImplRef>, OperationError<D::Error>> {
        let mut impls = UniqueVec::new();
        for candidate in ty.reference_chain() {
            rg_std::check_cancel!(self.context, "implementation candidates");
            for ty in candidate.as_adts() {
                rg_std::check_cancel!(self.context, "implementation candidates");
                for impl_ref in self.impls_for_type_def(ty.def)? {
                    rg_std::check_cancel!(self.context, "implementation candidates");
                    impls.push(impl_ref);
                }
            }
        }
        Ok(impls)
    }

    /// Returns impl blocks whose resolved self type mentions this nominal type definition.
    pub fn impls_for_type_def(
        &self,
        ty: TypeDefRef,
    ) -> Result<UniqueVec<ImplRef>, OperationError<D::Error>> {
        let mut impls = UniqueVec::new();
        for candidate in self.context.item_lookup().impls_for_type(ty) {
            rg_std::check_cancel!(self.context, "implementation candidates");
            impls.push(candidate);
        }
        rg_std::check_cancel!(self.context, "implementation candidates");
        Ok(impls)
    }

    /// Return all impls of a trait in the use-site crate and its supplied body scope.
    ///
    /// Trait-name navigation and method navigation must see the same declarations. The body
    /// resolver owns local-store discovery, including enclosing body-local modules; this query
    /// only merges those candidates with the saved crate indexes.
    pub fn impls_for_trait(
        &self,
        trait_ref: TraitDefRef,
        scope: Option<&impl SolverScope<Error = D::Error>>,
    ) -> Result<UniqueVec<ImplRef>, OperationError<D::Error>> {
        let mut impls = UniqueVec::new();
        for candidate in self.context.item_lookup().impls_for_trait(trait_ref) {
            rg_std::check_cancel!(self.context, "implementation candidates");
            impls.push(candidate);
        }
        if let Some(scope) = scope {
            for candidate in scope
                .local_trait_impls(trait_ref)
                .map_err(OperationError::Source)?
            {
                rg_std::check_cancel!(self.context, "implementation candidates");
                impls.push(candidate.impl_ref);
            }
        }
        rg_std::check_cancel!(self.context, "implementation candidates");
        Ok(impls)
    }

    /// Return the explicit implementations of a function in the supplied lookup scope.
    ///
    /// Both `Named::name` and `name` inside `impl Named for User` identify the same trait
    /// member. Once that identity is known, every impl can contribute its written method,
    /// regardless of the receiver or generic arguments used to resolve the original call.
    /// An inherent method already is its implementation; a free function has no impls.
    pub fn function_implementations(
        &self,
        function: FunctionRef,
        scope: Option<&impl SolverScope<Error = D::Error>>,
    ) -> Result<UniqueVec<FunctionRef>, OperationError<D::Error>> {
        let items = self.context.item_paths().items();
        let Some(data) = items
            .function_data(function)
            .map_err(OperationError::Source)?
        else {
            return Ok(UniqueVec::new());
        };

        let trait_ref = match data.owner {
            ItemOwner::Trait(id) => TraitDefRef {
                origin: function.origin,
                id,
            },
            ItemOwner::Impl(id) => {
                let Some(implementation) = items
                    .impl_data(ImplRef {
                        origin: function.origin,
                        id,
                    })
                    .map_err(OperationError::Source)?
                else {
                    return Ok(UniqueVec::new());
                };
                if implementation.trait_ref.is_none() {
                    return Ok([function].into_iter().collect());
                }
                let Some(trait_ref) = implementation.resolved_trait_ref.as_option() else {
                    return Ok(UniqueVec::new());
                };
                *trait_ref
            }
            ItemOwner::Module(_) => return Ok(UniqueVec::new()),
        };

        let mut functions = UniqueVec::new();
        for impl_ref in self.impls_for_trait(trait_ref, scope)? {
            rg_std::check_cancel!(self.context, "implementation candidates");
            functions.extend(self.matching_impl_methods(impl_ref, data.name.as_str())?);
        }
        rg_std::check_cancel!(self.context, "implementation candidates");
        Ok(functions)
    }

    fn matching_impl_methods(
        &self,
        impl_ref: ImplRef,
        method_name: &str,
    ) -> Result<UniqueVec<FunctionRef>, OperationError<D::Error>> {
        let Some(data) = self
            .context
            .item_paths()
            .items()
            .impl_data(impl_ref)
            .map_err(OperationError::Source)?
        else {
            return Ok(UniqueVec::new());
        };

        let mut functions = UniqueVec::new();
        for function in data.functions() {
            rg_std::check_cancel!(self.context, "implementation candidates");
            let Some(function_data) = self
                .context
                .item_paths()
                .items()
                .function_data(function)
                .map_err(OperationError::Source)?
            else {
                continue;
            };
            if function_data.name.as_str() != method_name {
                continue;
            }
            functions.push(function);
        }
        rg_std::check_cancel!(self.context, "implementation candidates");
        Ok(functions)
    }
}
