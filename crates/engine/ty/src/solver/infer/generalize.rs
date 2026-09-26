//! Instantiating a variable without creating a cycle or leaking a placeholder universe.
//!
//! Before storing `?T = SomeType`, walk `SomeType` and check what it contains. `?T = Vec<?T>`
//! would be an infinite type, so it must fail. An associated type is different: it can mention
//! `?T` and still normalize to a finite answer, so it may need a fresh variable and a goal to
//! solve later. A variable must also not capture placeholders introduced in a newer binder.
//! Each variable's universe records which of those placeholders it is allowed to name.
//!
//! Adapted from rust-analyzer's next_solver/infer/relate/generalize.rs at
//! aaddfb73fd95f2c0bf001b474dca91ae28bcce3a (MIT OR Apache-2.0).
//! Upstream: https://github.com/rust-lang/rust-analyzer

// The compiler relation API fixes this error type; boxing it would break that contract.
#![allow(clippy::result_large_err)]

use rustc_type_ir::{
    self as ir, InferCtxtLike, Interner as _, TypeVisitableExt,
    data_structures::HashMap,
    inherent::{Const as _, IntoKind, Ty as _},
    relate::{
        self, Relate, RelateResult, StructurallyRelateAliases, TypeRelation, VarianceDiagInfo,
        combine::PredicateEmittingRelation,
    },
};

use super::{InferCtxt, Value};
use crate::solver::{Const, DefId, GenericArgs, Region, SolverInterner, Term, Ty};

type I<'s> = SolverInterner<'s>;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Ty(ir::TyVid),
    Const(ir::ConstVid),
}

impl<'s> InferCtxt<'s> {
    pub(super) fn instantiate_ty_var<R: PredicateEmittingRelation<Self>>(
        &self,
        relation: &mut R,
        expected: bool,
        target: ir::TyVid,
        variance: ir::Variance,
        source: Ty<'s>,
    ) -> RelateResult<I<'s>, ()> {
        let universe = self
            .universe_of_ty(target)
            .expect("target type variable is unresolved");
        let (ty, unconstrained) = self.generalize(
            relation.structurally_relate_aliases(),
            Target::Ty(self.root_ty_var(target)),
            universe,
            variance,
            source,
        )?;
        if let ir::Infer(ir::TyVar(v)) = ty.kind() {
            self.equate_ty_vids_raw(target, v);
        } else {
            self.assign_ty(target, ty);
        }
        if unconstrained {
            relation.register_predicates([ir::ClauseKind::WellFormed(ty.into())]);
        }

        // `<?T as Trait>::Item = ?T` can be valid after normalization. Generalizing the alias
        // to a fresh variable delays that equation instead of eagerly reporting an occurs error.
        if ty.is_ty_var() {
            let (lhs, rhs, direction) = match variance {
                ir::Invariant => (ty.into(), source.into(), ir::AliasRelationDirection::Equate),
                ir::Covariant => (
                    ty.into(),
                    source.into(),
                    ir::AliasRelationDirection::Subtype,
                ),
                ir::Contravariant => (
                    source.into(),
                    ty.into(),
                    ir::AliasRelationDirection::Subtype,
                ),
                ir::Bivariant => unreachable!("bivariant variables are not instantiated"),
            };
            relation.register_predicates([ir::PredicateKind::AliasRelate(lhs, rhs, direction)]);
        } else if expected {
            relation.relate(ty, source)?;
        } else {
            relation.relate(source, ty)?;
        }
        Ok(())
    }

    pub(super) fn instantiate_const_var<R: PredicateEmittingRelation<Self>>(
        &self,
        relation: &mut R,
        expected: bool,
        target: ir::ConstVid,
        source: Const<'s>,
    ) -> RelateResult<I<'s>, ()> {
        let universe = self
            .universe_of_ct(target)
            .expect("target const variable is unresolved");
        let (ct, unconstrained) = self.generalize(
            relation.structurally_relate_aliases(),
            Target::Const(self.root_const_var(target)),
            universe,
            ir::Invariant,
            source,
        )?;
        debug_assert!(!unconstrained);
        self.assign_const(target, ct);
        let (a, b) = if expected { (ct, source) } else { (source, ct) };
        relation.relate_with_variance(ir::Invariant, VarianceDiagInfo::default(), a, b)?;
        Ok(())
    }

    fn generalize<T: Into<Term<'s>> + Relate<I<'s>>>(
        &self,
        aliases: StructurallyRelateAliases,
        target: Target,
        universe: ir::UniverseIndex,
        variance: ir::Variance,
        source: T,
    ) -> RelateResult<I<'s>, (T, bool)> {
        assert!(!source.has_escaping_bound_vars());
        let mut generalizer = Generalizer {
            infcx: self,
            aliases,
            target,
            universe,
            variance,
            term: source.into(),
            in_alias: false,
            cache: HashMap::default(),
            unconstrained: false,
        };
        // Relating the source to itself uses the compiler's traversal of type structure and
        // variance. The custom callbacks below replace parts that cannot be stored directly;
        // this is not an equality check between two different types.
        let value = generalizer.relate(source, source)?;
        Ok((value, generalizer.unconstrained))
    }
}

/// Builds a value that the target variable can hold, leaving further relations to the caller.
/// Fresh variables stand in for parts that still need solving; seeing the target variable again
/// in ordinary type structure is a cycle instead.
struct Generalizer<'a, 's> {
    infcx: &'a InferCtxt<'s>,
    aliases: StructurallyRelateAliases,
    target: Target,
    universe: ir::UniverseIndex,
    variance: ir::Variance,
    term: Term<'s>,
    in_alias: bool,
    cache: HashMap<(Ty<'s>, ir::Variance, bool), Ty<'s>>,
    unconstrained: bool,
}

impl<'s> Generalizer<'_, 's> {
    fn cyclic_error(&self) -> ir::error::TypeError<I<'s>> {
        match self.term.kind() {
            ir::TermKind::Ty(t) => ir::error::TypeError::CyclicTy(t),
            ir::TermKind::Const(c) => ir::error::TypeError::CyclicConst(c),
        }
    }

    fn fresh_alias_var(&mut self) -> Ty<'s> {
        self.unconstrained |= self.variance == ir::Bivariant;
        self.infcx.next_ty_var_in_universe(self.universe)
    }
}

impl<'s> TypeRelation<I<'s>> for Generalizer<'_, 's> {
    fn cx(&self) -> I<'s> {
        self.infcx.interner
    }

    fn relate_ty_args(
        &mut self,
        ty: Ty<'s>,
        _: Ty<'s>,
        def: DefId,
        a: GenericArgs<'s>,
        b: GenericArgs<'s>,
        mk: impl FnOnce(GenericArgs<'s>) -> Ty<'s>,
    ) -> RelateResult<I<'s>, Ty<'s>> {
        let args = if self.variance == ir::Invariant {
            relate::relate_args_invariantly(self, a, b)
        } else {
            relate::relate_args_with_variances(self, self.cx().variances_of(def), a, b)
        }?;
        Ok(if args == a { ty } else { mk(args) })
    }

    fn relate_with_variance<T: Relate<I<'s>>>(
        &mut self,
        variance: ir::Variance,
        _: VarianceDiagInfo<I<'s>>,
        a: T,
        b: T,
    ) -> RelateResult<I<'s>, T> {
        let previous = self.variance;
        self.variance = previous.xform(variance);
        let result = self.relate(a, b);
        self.variance = previous;
        result
    }

    fn tys(&mut self, a: Ty<'s>, b: Ty<'s>) -> RelateResult<I<'s>, Ty<'s>> {
        assert_eq!(a, b);
        let key = (a, self.variance, self.in_alias);
        if let Some(value) = self.cache.get(&key) {
            return Ok(*value);
        }
        let result = match a.kind() {
            ir::Infer(ir::TyVar(v)) => {
                let v = self.infcx.root_ty_var(v);
                if Target::Ty(v) == self.target {
                    return Err(self.cyclic_error());
                }
                match self.infcx.ty_value(v) {
                    Value::Known(ty) => self.relate(ty, ty)?,
                    Value::Unknown(universe) => {
                        if self.variance == ir::Invariant && self.universe.can_name(universe) {
                            return Ok(a);
                        }
                        self.unconstrained |= self.variance == ir::Bivariant;
                        let fresh = self.infcx.next_ty_var_in_universe(self.universe);
                        if self.in_alias && !self.infcx.typing_mode_raw().is_coherence() {
                            let ir::Infer(ir::TyVar(new)) = fresh.kind() else {
                                unreachable!()
                            };
                            self.infcx.equate_ty_vids_raw(v, new);
                        }
                        fresh
                    }
                }
            }
            ir::Infer(ir::IntVar(_) | ir::FloatVar(_)) => a,
            ir::Infer(_) => unreachable!("freshening variables are not used by the next solver"),
            ir::Placeholder(p) => {
                if !self.universe.can_name(p.universe) {
                    return Err(ir::error::TypeError::Mismatch);
                }
                a
            }
            ir::Alias(alias) if matches!(self.aliases, StructurallyRelateAliases::No) => {
                if !alias.has_escaping_bound_vars() && !self.in_alias {
                    self.fresh_alias_var()
                } else {
                    let nested = std::mem::replace(&mut self.in_alias, true);
                    let result = self.relate(alias, alias);
                    self.in_alias = nested;
                    match result {
                        Ok(alias) => alias.to_ty(self.cx()),
                        Err(error) if nested => return Err(error),
                        Err(_) => {
                            // This is the same incomplete higher-ranked alias case as upstream.
                            // Keep the uncertainty visible rather than treating it as a proof.
                            self.cx()
                                .unavailable("generalizing an alias with escaping bound variables");
                            self.fresh_alias_var()
                        }
                    }
                }
            }
            _ => relate::structurally_relate_tys(self, a, a)?,
        };
        self.cache.insert(key, result);
        Ok(result)
    }

    fn consts(&mut self, a: Const<'s>, b: Const<'s>) -> RelateResult<I<'s>, Const<'s>> {
        assert_eq!(a, b);
        match a.kind() {
            ir::ConstKind::Infer(ir::InferConst::Var(v)) => {
                let v = self.infcx.root_const_var(v);
                if Target::Const(v) == self.target {
                    return Err(self.cyclic_error());
                }
                match self.infcx.const_value(v) {
                    Value::Known(c) => self.relate(c, c),
                    Value::Unknown(universe) if self.universe.can_name(universe) => Ok(a),
                    Value::Unknown(_) => {
                        let c = self.infcx.next_const_var_in_universe(self.universe);
                        if self.in_alias && !self.infcx.typing_mode_raw().is_coherence() {
                            let ir::ConstKind::Infer(ir::InferConst::Var(new)) = c.kind() else {
                                unreachable!()
                            };
                            self.infcx.equate_const_vids_raw(v, new);
                        }
                        Ok(c)
                    }
                }
            }
            ir::ConstKind::Placeholder(p) if !self.universe.can_name(p.universe) => {
                Err(ir::error::TypeError::Mismatch)
            }
            ir::ConstKind::Unevaluated(ir::UnevaluatedConst { def, args }) => {
                let args = self.relate_with_variance(
                    ir::Invariant,
                    VarianceDiagInfo::default(),
                    args,
                    args,
                )?;
                Ok(Const::new_unevaluated(
                    self.cx(),
                    ir::UnevaluatedConst { def, args },
                ))
            }
            _ => relate::structurally_relate_consts(self, a, a),
        }
    }

    fn regions(&mut self, a: Region<'s>, b: Region<'s>) -> RelateResult<I<'s>, Region<'s>> {
        assert_eq!(a, b);
        if matches!(a.kind(), ir::ReBound(..) | ir::ReErased | ir::ReError(_))
            || self.variance == ir::Invariant
                && self.universe.can_name(self.infcx.universe_of_region(a))
        {
            Ok(a)
        } else {
            Ok(self.infcx.next_region_var_in_universe(self.universe))
        }
    }

    fn binders<T: Relate<I<'s>>>(
        &mut self,
        a: ir::Binder<I<'s>, T>,
        _: ir::Binder<I<'s>, T>,
    ) -> RelateResult<I<'s>, ir::Binder<I<'s>, T>> {
        let result = self.relate(a.skip_binder(), a.skip_binder())?;
        Ok(a.rebind(result))
    }
}
