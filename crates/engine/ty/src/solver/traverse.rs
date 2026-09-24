//! Traversal of scoped types, following rustc's binder rules.
//!
//! A visitor inspects a node; a folder can replace it. Their `super_*` methods below continue
//! into that node's children, so a custom callback can handle variables itself and delegate the
//! rest of the structure here. Calling the node's own callback again would recurse forever.
//!
//! Adapted from rust-analyzer's next_solver/{ty,consts,predicate}.rs at
//! aaddfb73fd95f2c0bf001b474dca91ae28bcce3a (MIT OR Apache-2.0).
//! Upstream: https://github.com/rust-lang/rust-analyzer

use rustc_type_ir::{
    self as ir, TypeFoldable, TypeSuperFoldable, TypeSuperVisitable, TypeVisitable, VisitorResult,
    inherent::IntoKind,
};

use super::{Const, Predicate, SolverInterner as Interner, Ty, types::TyKind};
type ConstKind<'s> = ir::ConstKind<Interner<'s>>;

macro_rules! try_visit {
    ($v:expr) => {
        if let std::ops::ControlFlow::Break(r) = $v.branch() {
            return V::Result::from_residual(r);
        }
    };
}

impl<'s> TypeSuperVisitable<Interner<'s>> for Ty<'s> {
    fn super_visit_with<V: rustc_type_ir::TypeVisitor<Interner<'s>>>(
        &self,
        visitor: &mut V,
    ) -> V::Result {
        match (*self).kind() {
            TyKind::RawPtr(ty, _mutbl) => ty.visit_with(visitor),
            TyKind::Array(typ, sz) => {
                try_visit!(typ.visit_with(visitor));
                sz.visit_with(visitor)
            }
            TyKind::Slice(typ) => typ.visit_with(visitor),
            TyKind::Adt(_, args) => args.visit_with(visitor),
            TyKind::Dynamic(ref trait_ty, ref reg) => {
                try_visit!(trait_ty.visit_with(visitor));
                reg.visit_with(visitor)
            }
            TyKind::Tuple(ts) => ts.visit_with(visitor),
            TyKind::FnDef(_, args) => args.visit_with(visitor),
            TyKind::FnPtr(ref sig_tys, _) => sig_tys.visit_with(visitor),
            TyKind::UnsafeBinder(f) => f.visit_with(visitor),
            TyKind::Ref(r, ty, _) => {
                try_visit!(r.visit_with(visitor));
                ty.visit_with(visitor)
            }
            TyKind::Coroutine(_did, ref args) => args.visit_with(visitor),
            TyKind::CoroutineWitness(_did, ref args) => args.visit_with(visitor),
            TyKind::Closure(_did, ref args) => args.visit_with(visitor),
            TyKind::CoroutineClosure(_did, ref args) => args.visit_with(visitor),
            TyKind::Alias(ref data) => data.visit_with(visitor),

            TyKind::Pat(ty, pat) => {
                try_visit!(ty.visit_with(visitor));
                pat.visit_with(visitor)
            }

            TyKind::Error(guar) => guar.visit_with(visitor),

            TyKind::Bool
            | TyKind::Char
            | TyKind::Str
            | TyKind::Int(_)
            | TyKind::Uint(_)
            | TyKind::Float(_)
            | TyKind::Infer(_)
            | TyKind::Bound(..)
            | TyKind::Placeholder(..)
            | TyKind::Param(..)
            | TyKind::Never
            | TyKind::Foreign(..) => V::Result::output(),
        }
    }
}

impl<'s> TypeSuperFoldable<Interner<'s>> for Ty<'s> {
    fn try_super_fold_with<F: rustc_type_ir::FallibleTypeFolder<Interner<'s>>>(
        self,
        folder: &mut F,
    ) -> Result<Self, F::Error> {
        let kind = match self.kind() {
            TyKind::RawPtr(ty, mutbl) => TyKind::RawPtr(ty.try_fold_with(folder)?, mutbl),
            TyKind::Array(typ, sz) => {
                TyKind::Array(typ.try_fold_with(folder)?, sz.try_fold_with(folder)?)
            }
            TyKind::Slice(typ) => TyKind::Slice(typ.try_fold_with(folder)?),
            TyKind::Adt(tid, args) => TyKind::Adt(tid, args.try_fold_with(folder)?),
            TyKind::Dynamic(trait_ty, region) => TyKind::Dynamic(
                trait_ty.try_fold_with(folder)?,
                region.try_fold_with(folder)?,
            ),
            TyKind::Tuple(ts) => TyKind::Tuple(ts.try_fold_with(folder)?),
            TyKind::FnDef(def_id, args) => TyKind::FnDef(def_id, args.try_fold_with(folder)?),
            TyKind::FnPtr(sig_tys, hdr) => TyKind::FnPtr(sig_tys.try_fold_with(folder)?, hdr),
            TyKind::UnsafeBinder(f) => TyKind::UnsafeBinder(f.try_fold_with(folder)?),
            TyKind::Ref(r, ty, mutbl) => {
                TyKind::Ref(r.try_fold_with(folder)?, ty.try_fold_with(folder)?, mutbl)
            }
            TyKind::Coroutine(did, args) => TyKind::Coroutine(did, args.try_fold_with(folder)?),
            TyKind::CoroutineWitness(did, args) => {
                TyKind::CoroutineWitness(did, args.try_fold_with(folder)?)
            }
            TyKind::Closure(did, args) => TyKind::Closure(did, args.try_fold_with(folder)?),
            TyKind::CoroutineClosure(did, args) => {
                TyKind::CoroutineClosure(did, args.try_fold_with(folder)?)
            }
            TyKind::Alias(data) => TyKind::Alias(data.try_fold_with(folder)?),
            TyKind::Pat(ty, pat) => {
                TyKind::Pat(ty.try_fold_with(folder)?, pat.try_fold_with(folder)?)
            }

            TyKind::Bool
            | TyKind::Char
            | TyKind::Str
            | TyKind::Int(_)
            | TyKind::Uint(_)
            | TyKind::Float(_)
            | TyKind::Error(_)
            | TyKind::Infer(_)
            | TyKind::Param(..)
            | TyKind::Bound(..)
            | TyKind::Placeholder(..)
            | TyKind::Never
            | TyKind::Foreign(..) => return Ok(self),
        };

        Ok(if self.kind() == kind {
            self
        } else {
            Ty::new(folder.cx(), kind)
        })
    }

    fn super_fold_with<F: rustc_type_ir::TypeFolder<Interner<'s>>>(self, folder: &mut F) -> Self {
        let kind = match self.kind() {
            TyKind::RawPtr(ty, mutbl) => TyKind::RawPtr(ty.fold_with(folder), mutbl),
            TyKind::Array(typ, sz) => TyKind::Array(typ.fold_with(folder), sz.fold_with(folder)),
            TyKind::Slice(typ) => TyKind::Slice(typ.fold_with(folder)),
            TyKind::Adt(tid, args) => TyKind::Adt(tid, args.fold_with(folder)),
            TyKind::Dynamic(trait_ty, region) => {
                TyKind::Dynamic(trait_ty.fold_with(folder), region.fold_with(folder))
            }
            TyKind::Tuple(ts) => TyKind::Tuple(ts.fold_with(folder)),
            TyKind::FnDef(def_id, args) => TyKind::FnDef(def_id, args.fold_with(folder)),
            TyKind::FnPtr(sig_tys, hdr) => TyKind::FnPtr(sig_tys.fold_with(folder), hdr),
            TyKind::UnsafeBinder(f) => TyKind::UnsafeBinder(f.fold_with(folder)),
            TyKind::Ref(r, ty, mutbl) => {
                TyKind::Ref(r.fold_with(folder), ty.fold_with(folder), mutbl)
            }
            TyKind::Coroutine(did, args) => TyKind::Coroutine(did, args.fold_with(folder)),
            TyKind::CoroutineWitness(did, args) => {
                TyKind::CoroutineWitness(did, args.fold_with(folder))
            }
            TyKind::Closure(did, args) => TyKind::Closure(did, args.fold_with(folder)),
            TyKind::CoroutineClosure(did, args) => {
                TyKind::CoroutineClosure(did, args.fold_with(folder))
            }
            TyKind::Alias(data) => TyKind::Alias(data.fold_with(folder)),
            TyKind::Pat(ty, pat) => TyKind::Pat(ty.fold_with(folder), pat.fold_with(folder)),

            TyKind::Bool
            | TyKind::Char
            | TyKind::Str
            | TyKind::Int(_)
            | TyKind::Uint(_)
            | TyKind::Float(_)
            | TyKind::Error(_)
            | TyKind::Infer(_)
            | TyKind::Param(..)
            | TyKind::Bound(..)
            | TyKind::Placeholder(..)
            | TyKind::Never
            | TyKind::Foreign(..) => return self,
        };

        if self.kind() == kind {
            self
        } else {
            Ty::new(folder.cx(), kind)
        }
    }
}

impl<'s> TypeSuperVisitable<Interner<'s>> for Const<'s> {
    fn super_visit_with<V: rustc_type_ir::TypeVisitor<Interner<'s>>>(
        &self,
        visitor: &mut V,
    ) -> V::Result {
        match self.kind() {
            ConstKind::Unevaluated(uv) => uv.visit_with(visitor),
            ConstKind::Value(v) => v.visit_with(visitor),
            ConstKind::Expr(e) => e.visit_with(visitor),
            ConstKind::Error(e) => e.visit_with(visitor),

            ConstKind::Param(_)
            | ConstKind::Infer(_)
            | ConstKind::Bound(..)
            | ConstKind::Placeholder(_) => V::Result::output(),
        }
    }
}

impl<'s> TypeSuperFoldable<Interner<'s>> for Const<'s> {
    fn try_super_fold_with<F: rustc_type_ir::FallibleTypeFolder<Interner<'s>>>(
        self,
        folder: &mut F,
    ) -> Result<Self, F::Error> {
        let kind = match self.kind() {
            ConstKind::Unevaluated(uv) => ConstKind::Unevaluated(uv.try_fold_with(folder)?),
            ConstKind::Value(v) => ConstKind::Value(v.try_fold_with(folder)?),
            ConstKind::Expr(e) => match e {},

            ConstKind::Param(_)
            | ConstKind::Infer(_)
            | ConstKind::Bound(..)
            | ConstKind::Placeholder(_)
            | ConstKind::Error(_) => return Ok(self),
        };
        if kind != self.kind() {
            Ok(Const::new(folder.cx(), kind))
        } else {
            Ok(self)
        }
    }

    fn super_fold_with<F: rustc_type_ir::TypeFolder<Interner<'s>>>(self, folder: &mut F) -> Self {
        let kind = match self.kind() {
            ConstKind::Unevaluated(uv) => ConstKind::Unevaluated(uv.fold_with(folder)),
            ConstKind::Value(v) => ConstKind::Value(v.fold_with(folder)),
            ConstKind::Expr(e) => match e {},

            ConstKind::Param(_)
            | ConstKind::Infer(_)
            | ConstKind::Bound(..)
            | ConstKind::Placeholder(_)
            | ConstKind::Error(_) => return self,
        };
        if kind != self.kind() {
            Const::new(folder.cx(), kind)
        } else {
            self
        }
    }
}

impl<'s> TypeSuperVisitable<Interner<'s>> for Predicate<'s> {
    fn super_visit_with<V: rustc_type_ir::TypeVisitor<Interner<'s>>>(
        &self,
        visitor: &mut V,
    ) -> V::Result {
        (*self).kind().visit_with(visitor)
    }
}

impl<'s> TypeSuperFoldable<Interner<'s>> for Predicate<'s> {
    fn try_super_fold_with<F: rustc_type_ir::FallibleTypeFolder<Interner<'s>>>(
        self,
        folder: &mut F,
    ) -> Result<Self, F::Error> {
        let new = self.kind().try_fold_with(folder)?;
        Ok(Predicate::new(folder.cx(), new))
    }

    fn super_fold_with<F: rustc_type_ir::TypeFolder<Interner<'s>>>(self, folder: &mut F) -> Self {
        let new = self.kind().fold_with(folder);
        Predicate::new(folder.cx(), new)
    }
}
