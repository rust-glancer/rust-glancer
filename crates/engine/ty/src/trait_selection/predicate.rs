//! Small local predicate proof before asking Chalk.
//!
//! Chalk is still the owner for ordinary impl predicates. This module handles one source of
//! evidence that does not cross the Chalk boundary yet: opaque `impl Trait` bounds already stored
//! in `InferTy`. For example, after matching
//!
//! ```text
//! impl<I: Iterator> IntoIterator for I
//! ```
//!
//! against `impl Iterator<Item = User>`, the substituted predicate is exactly the declared opaque
//! bound. Proving that locally lets selection reach the associated projection path without making
//! the Chalk adapter pretend it has real opaque-type data.

use rg_ir_model::hir::items::ImplData;
use rg_ir_model::items::{TypeBound, TypeRef, WherePredicate};
use rg_ir_model::{Path, TraitApplicability, TraitRef, TypePathResolution};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TargetItemQuery, TypePathContext};
use rg_std::ExpectedUnique;

use crate::ItemPathQuery;
use crate::inference::{InferTy, InferTypeSubst, InferenceTable};

pub(super) enum ImplPredicateProof {
    Proven(TraitApplicability),
    Rejected,
    NotApplicable,
}

pub(super) struct ImplPredicateProver<'prover, 'query, D, I> {
    item_paths: &'prover ItemPathQuery<'query, D, I>,
    target_items: &'prover TargetItemQuery<'query, D, I>,
}

impl<'prover, 'query, D, I> ImplPredicateProver<'prover, 'query, D, I>
where
    D: DefMapSource<Error = I::Error>,
    I: ItemStoreSource<'query>,
{
    pub(super) fn new(
        item_paths: &'prover ItemPathQuery<'query, D, I>,
        target_items: &'prover TargetItemQuery<'query, D, I>,
    ) -> Self {
        Self {
            item_paths,
            target_items,
        }
    }

    pub(super) fn prove_all_from_opaque_bounds(
        &self,
        impl_data: &ImplData,
        subst: &InferTypeSubst,
        table: &InferenceTable,
    ) -> Result<ImplPredicateProof, I::Error> {
        let context = TypePathContext {
            module: impl_data.owner,
            impl_ref: None,
        };
        let mut proved_any = false;

        for param in &impl_data.generics.types {
            let Some(subject) = subst
                .type_param(param.name.as_str())
                .map(|ty| table.canonicalize(&ty))
            else {
                continue;
            };
            for bound in &param.bounds {
                match self.prove_trait_bound(context, &subject, bound)? {
                    SinglePredicateProof::Proven => proved_any = true,
                    SinglePredicateProof::Rejected => return Ok(ImplPredicateProof::Rejected),
                    SinglePredicateProof::NotApplicable => {
                        return Ok(ImplPredicateProof::NotApplicable);
                    }
                }
            }
        }

        for predicate in &impl_data.generics.where_predicates {
            let WherePredicate::Type { ty, bounds } = predicate else {
                return Ok(ImplPredicateProof::NotApplicable);
            };
            let Some(subject) = self.type_param_subject(ty, subst, table) else {
                return Ok(ImplPredicateProof::NotApplicable);
            };
            for bound in bounds {
                match self.prove_trait_bound(context, &subject, bound)? {
                    SinglePredicateProof::Proven => proved_any = true,
                    SinglePredicateProof::Rejected => return Ok(ImplPredicateProof::Rejected),
                    SinglePredicateProof::NotApplicable => {
                        return Ok(ImplPredicateProof::NotApplicable);
                    }
                }
            }
        }

        if proved_any {
            Ok(ImplPredicateProof::Proven(TraitApplicability::Yes))
        } else {
            Ok(ImplPredicateProof::NotApplicable)
        }
    }

    fn type_param_subject(
        &self,
        ty: &TypeRef,
        subst: &InferTypeSubst,
        table: &InferenceTable,
    ) -> Option<InferTy> {
        let name = ty.type_param_name()?;
        subst
            .type_param(name.as_str())
            .map(|ty| table.canonicalize(&ty))
    }

    fn prove_trait_bound(
        &self,
        context: TypePathContext,
        subject: &InferTy,
        bound: &TypeBound,
    ) -> Result<SinglePredicateProof, I::Error> {
        let InferTy::Opaque { bounds } = subject else {
            return Ok(SinglePredicateProof::NotApplicable);
        };
        let Some(trait_ref) = self.empty_trait_bound_ref(context, bound)? else {
            return Ok(SinglePredicateProof::NotApplicable);
        };

        let mut matching_bounds = 0usize;
        for bound in bounds {
            if bound.trait_ref == trait_ref {
                matching_bounds += 1;
            }
        }

        Ok(match matching_bounds {
            0 => SinglePredicateProof::Rejected,
            1 => SinglePredicateProof::Proven,
            _ => SinglePredicateProof::NotApplicable,
        })
    }

    fn empty_trait_bound_ref(
        &self,
        context: TypePathContext,
        bound: &TypeBound,
    ) -> Result<Option<TraitRef>, I::Error> {
        let TypeBound::Trait(TypeRef::Path(path)) = bound else {
            return Ok(None);
        };
        let Some(segment) = path.segments.last() else {
            return Ok(None);
        };
        if !segment.args.is_empty() {
            return Ok(None);
        }
        if let TypePathResolution::Trait(trait_ref) = self
            .item_paths
            .resolve_type_path(context, &Path::from_type_path(path))?
        {
            return Ok(Some(trait_ref));
        }
        let Some(name) = path.single_name() else {
            return Ok(None);
        };

        // Fixture and generated contexts sometimes preserve a plain trait name even when the
        // source module cannot resolve it. Chalk already has the same unique-name escape hatch;
        // keep this local proof aligned so opaque bounds do not fall through only because of test
        // or generated-code path shape.
        //
        // TODO: make the resolver tell us when this fallback is allowed. Production source paths
        // should eventually fail closed when the trait path did not resolve in scope.
        let mut traits = ExpectedUnique::new();
        for store in self.target_items.visible_stores()? {
            for (trait_ref, trait_data) in store.traits_with_refs() {
                if trait_data.name.as_str() == name.as_str() {
                    traits.push(trait_ref);
                }
            }
        }

        Ok(traits.into_option())
    }
}

enum SinglePredicateProof {
    Proven,
    Rejected,
    NotApplicable,
}
