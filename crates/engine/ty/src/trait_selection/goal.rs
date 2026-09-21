//! Trait goals and the identities used to route and reuse their proofs.

use crate::{
    AssocTypeBinding, GenericArg, GenericArgs, TraitApplication, TraitRefLowering, Ty,
    inference::InferenceTable,
};
use rg_ir_model::{CrateRef, TypeAliasRef};
use rg_std::UniqueVec;

/// A canonical trait application plus any associated-type equality constraints.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraitGoal {
    pub application: TraitApplication,
    pub associated_types: Vec<AssocTypeBinding>,
}

impl TraitGoal {
    /// Build a goal from positional arguments that do not include `Self`.
    pub fn new(
        self_ty: Ty,
        trait_ref: rg_ir_model::TraitDefRef,
        args: impl Into<GenericArgs>,
    ) -> Self {
        let args = args.into();
        let mut full_args = Vec::with_capacity(1 + args.len());
        full_args.push(GenericArg::Type(Box::new(self_ty)));
        full_args.extend(args.into_vec());
        Self {
            application: TraitApplication {
                def: trait_ref,
                args: full_args.into(),
            },
            associated_types: Vec::new(),
        }
    }

    pub fn from_lowering(lowering: TraitRefLowering) -> Self {
        Self {
            application: lowering.application,
            associated_types: lowering.associated_types,
        }
    }

    pub fn self_ty(&self) -> &Ty {
        self.application
            .self_ty()
            .expect("trait applications always contain the Self argument")
    }

    pub fn trait_ref(&self) -> rg_ir_model::TraitDefRef {
        self.application.def
    }

    /// Iterate trait input args without associated-type equality constraints.
    ///
    /// Rust syntax puts both shapes inside the same angle brackets:
    ///
    /// ```text
    /// Iterator<Item = User>
    /// Indexed<Key, Item = User>
    /// ```
    ///
    /// Only the positional inputs belong in the trait substitution that Chalk sees as
    /// `Implemented(Self: Trait<...>)`. Associated equality args are separate projection
    /// constraints, such as `<Self as Iterator>::Item = User`.
    pub fn iter_positional_args(&self) -> impl Iterator<Item = &GenericArg> {
        self.application.args.iter().skip(1)
    }

    /// Return the crates that can possibly own an impl for this fully known application.
    ///
    /// Rust coherence requires the impl crate to own either the trait or a local type participating
    /// in its application. We deliberately recurse through every known type constructor instead of
    /// reproducing the exact fundamental-type rules here. That over-approximation may retain an
    /// impossible candidate, but it cannot discard a legal one.
    ///
    /// An unresolved type disables the optimization. In particular, a later solution for a type
    /// parameter or projection can introduce an owning crate that is not visible in the current
    /// shape, so filtering that goal would turn incomplete inference into a negative proof.
    pub(crate) fn possible_impl_origins(
        &self,
        table: &InferenceTable,
    ) -> Option<UniqueVec<CrateRef>> {
        let mut origins = UniqueVec::new();
        origins.push(self.trait_ref().origin.origin_crate());

        for arg in &self.application.args {
            let GenericArg::Type(ty) = arg else {
                continue;
            };
            let ty = table.canonicalize(ty);
            if !Self::collect_possible_impl_origins(&ty, &mut origins) {
                return None;
            }
        }
        Some(origins)
    }

    fn collect_possible_impl_origins(ty: &Ty, origins: &mut UniqueVec<CrateRef>) -> bool {
        match ty {
            Ty::Unit | Ty::Never | Ty::Primitive(_) => true,
            Ty::Tuple(fields) => fields
                .iter()
                .all(|field| Self::collect_possible_impl_origins(field, origins)),
            Ty::Array { inner, .. }
            | Ty::Slice(inner)
            | Ty::Reference { inner, .. }
            | Ty::RawPointer { inner, .. } => Self::collect_possible_impl_origins(inner, origins),
            Ty::FnPointer { params, ret } => {
                params
                    .iter()
                    .all(|param| Self::collect_possible_impl_origins(param, origins))
                    && Self::collect_possible_impl_origins(ret, origins)
            }
            Ty::Adt(adt) => {
                origins.push(adt.def.origin.origin_crate());
                adt.args.iter().all(|arg| match arg {
                    GenericArg::Type(ty) => Self::collect_possible_impl_origins(ty, origins),
                    GenericArg::Lifetime(_) | GenericArg::Const(_) => true,
                })
            }
            Ty::Param(_)
            | Ty::Alias(_)
            | Ty::Closure(_)
            | Ty::FnDef(_)
            | Ty::Unknown
            | Ty::InferVar { .. } => false,
        }
    }

    pub(crate) fn without_assoc_type_constraints(&self) -> Self {
        Self {
            application: self.application.clone(),
            associated_types: Vec::new(),
        }
    }

    pub(crate) fn has_assoc_type_constraints(&self) -> bool {
        !self.associated_types.is_empty()
    }

    pub(crate) fn assoc_type_constraints(&self) -> impl Iterator<Item = AssocTypeConstraint<'_>> {
        self.associated_types
            .iter()
            .map(|binding| AssocTypeConstraint {
                associated_ty: binding.associated_ty,
                ty: &binding.ty,
            })
    }

    /// Return whether this goal is independent of one body's live inference state.
    ///
    /// Semantic unknowns and projections are stable values: a later, more precise query produces a
    /// different goal. Inference variables and closure identities instead belong to the caller's
    /// table/body and must be classified again there.
    pub(crate) fn is_cache_stable(&self) -> bool {
        self.application
            .args
            .iter()
            .all(|arg| !arg.has_var() && !arg.has_closure())
            && self
                .associated_types
                .iter()
                .all(|binding| !binding.ty.has_var() && !binding.ty.has_closure())
    }
}

/// One `Trait<Assoc = Ty>` equality constraint carried by a trait goal.
pub(crate) struct AssocTypeConstraint<'a> {
    pub(crate) associated_ty: TypeAliasRef,
    pub(crate) ty: &'a Ty,
}
