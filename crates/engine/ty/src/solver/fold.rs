//! Replacing bound variables without capturing variables under nested binders.
//!
//! When replacing variables from an outer `for<...>`, a nested function type can introduce its
//! own `for<...>`. The depth tells us which binder a variable belongs to. We replace only the
//! requested binder's variables and shift replacements to keep their meaning under inner ones.
//!
//! The depth handling follows rust-analyzer's next_solver/fold.rs at
//! aaddfb73fd95f2c0bf001b474dca91ae28bcce3a (MIT OR Apache-2.0).
//! Upstream: https://github.com/rust-lang/rust-analyzer

use rustc_type_ir::{
    self as ir, TypeFoldable, TypeFolder, TypeSuperFoldable, TypeVisitableExt,
    data_structures::HashMap,
    inherent::{Const as _, GenericArg as _, IntoKind, Region as _, Ty as _},
};

use super::{Const, GenericArg, List, Region, SolverInterner, Ty};

struct BoundVarReplacer<'s, F> {
    cx: SolverInterner<'s>,
    depth: ir::DebruijnIndex,
    replace: F,
}

impl<'s, F> TypeFolder<SolverInterner<'s>> for BoundVarReplacer<'s, F>
where
    F: FnMut(ir::BoundVar, ir::BoundVariableKind<SolverInterner<'s>>) -> GenericArg<'s>,
{
    fn cx(&self) -> SolverInterner<'s> {
        self.cx
    }

    fn fold_binder<T: TypeFoldable<SolverInterner<'s>>>(
        &mut self,
        binder: ir::Binder<SolverInterner<'s>, T>,
    ) -> ir::Binder<SolverInterner<'s>, T> {
        self.depth.shift_in(1);
        let result = binder.super_fold_with(self);
        self.depth.shift_out(1);
        result
    }

    fn fold_ty(&mut self, ty: Ty<'s>) -> Ty<'s> {
        if let ir::Bound(ir::BoundVarIndexKind::Bound(depth), bound) = ty.kind()
            && depth == self.depth
        {
            let value =
                (self.replace)(bound.var, ir::BoundVariableKind::Ty(bound.kind)).expect_ty();
            ir::shift_vars(self.cx, value, self.depth.as_u32())
        } else if ty.has_vars_bound_at_or_above(self.depth) {
            ty.super_fold_with(self)
        } else {
            ty
        }
    }

    fn fold_const(&mut self, ct: Const<'s>) -> Const<'s> {
        if let ir::ConstKind::Bound(ir::BoundVarIndexKind::Bound(depth), bound) = ct.kind()
            && depth == self.depth
        {
            let value = (self.replace)(bound.var, ir::BoundVariableKind::Const).expect_const();
            ir::shift_vars(self.cx, value, self.depth.as_u32())
        } else {
            ct.super_fold_with(self)
        }
    }

    fn fold_region(&mut self, region: Region<'s>) -> Region<'s> {
        if let ir::ReBound(ir::BoundVarIndexKind::Bound(depth), bound) = region.kind()
            && depth == self.depth
        {
            let value = (self.replace)(bound.var, ir::BoundVariableKind::Region(bound.kind))
                .expect_region();
            ir::shift_vars(self.cx, value, self.depth.as_u32())
        } else {
            region
        }
    }
}

impl<'s> SolverInterner<'s> {
    pub(crate) fn replace_bound_vars<T: TypeFoldable<Self>>(
        self,
        value: ir::Binder<Self, T>,
        replace: impl FnMut(ir::BoundVar, ir::BoundVariableKind<Self>) -> GenericArg<'s>,
    ) -> T {
        value.skip_binder().fold_with(&mut BoundVarReplacer {
            cx: self,
            depth: ir::INNERMOST,
            replace,
        })
    }

    fn bound_arg(self, var: ir::BoundVar, kind: ir::BoundVariableKind<Self>) -> GenericArg<'s> {
        match kind {
            ir::BoundVariableKind::Ty(kind) => {
                Ty::new_bound(self, ir::INNERMOST, ir::BoundTy { var, kind }).into()
            }
            ir::BoundVariableKind::Region(kind) => {
                Region::new_bound(self, ir::INNERMOST, ir::BoundRegion { var, kind }).into()
            }
            ir::BoundVariableKind::Const => Const::new_anon_bound(self, ir::INNERMOST, var).into(),
        }
    }

    pub(crate) fn shift_bound_indices<T: TypeFoldable<Self>>(self, value: T, offset: u32) -> T {
        value.fold_with(&mut BoundVarReplacer {
            cx: self,
            depth: ir::INNERMOST,
            replace: |var: ir::BoundVar, kind| {
                self.bound_arg(ir::BoundVar::from_u32(var.as_u32() + offset), kind)
            },
        })
    }

    /// Remove names and unused slots from a binder. For example, `for<'a> fn(&'a str)` and
    /// `for<'b> fn(&'b str)` should have the same representation for comparison and caching.
    pub(crate) fn anonymize<T: TypeFoldable<Self>>(
        self,
        value: ir::Binder<Self, T>,
    ) -> ir::Binder<Self, T> {
        // Use first occurrence order, so differently named or ordered binders canonicalize to
        // the same value. Keep the vector separate from the map to make that order explicit.
        let mut indices = HashMap::default();
        let mut kinds = Vec::new();
        let inner = self.replace_bound_vars(value, |var, kind| {
            let kind = match kind {
                ir::BoundVariableKind::Ty(_) => ir::BoundVariableKind::Ty(ir::BoundTyKind::Anon),
                ir::BoundVariableKind::Region(_) => {
                    ir::BoundVariableKind::Region(ir::BoundRegionKind::Anon)
                }
                ir::BoundVariableKind::Const => ir::BoundVariableKind::Const,
            };
            let new_var = *indices.entry(var).or_insert_with(|| {
                let index = ir::BoundVar::from_usize(kinds.len());
                kinds.push(kind);
                index
            });
            self.bound_arg(new_var, kind)
        });
        ir::Binder::bind_with_vars(inner, List::new(self, &kinds))
    }
}
