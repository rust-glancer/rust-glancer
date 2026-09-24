//! Crossing between durable declaration/fact types and scoped solver values.
//!
//! Lowering keeps generic parameters tied to their source owner and copies the type shape into
//! solver storage. Instantiating those parameters and resolving variables happen in the inference
//! table. Raising copies a shape back out; it cannot consult variable assignments on its own, so
//! publishing an inferred result goes through the table's finalization first.

use rg_ir_model::GenericParamRef;
use rustc_type_ir::{
    self as ir, Interner as _, Upcast,
    inherent::{AdtDef as _, GenericArgs as _, IntoKind, Term as _, Ty as _},
};

use super::{
    Clause, Const, DefId, GenericArgs, List, Param, Region, SolverInterner, Ty,
    types::{ErrorGuaranteed, Safety, ValueConst},
};

impl<'s> SolverInterner<'s> {
    pub(crate) fn param(
        self,
        source: GenericParamRef,
        params: &[GenericParamRef],
    ) -> Option<Param> {
        let index = params.iter().position(|p| *p == source);
        if index.is_none() {
            self.unavailable("parameter outside its owner");
        }
        index.map(|index| Param {
            index: index as u32,
            source,
        })
    }

    pub fn lower_ty(self, ty: &crate::Ty, params: &[GenericParamRef]) -> Ty<'s> {
        use crate::Ty as Stored;
        let kind = match ty {
            Stored::Unit => ir::Tuple(List::default()),
            Stored::Never => ir::Never,
            Stored::Primitive(p) => return self.lower_primitive(*p),
            Stored::Tuple(fields) => ir::Tuple(List::new(
                self,
                &fields
                    .iter()
                    .map(|t| self.lower_ty(t, params))
                    .collect::<Vec<_>>(),
            )),
            Stored::Array { inner, len } => {
                ir::Array(self.lower_ty(inner, params), self.lower_const(*len, params))
            }
            Stored::Slice(inner) => ir::Slice(self.lower_ty(inner, params)),
            Stored::Reference {
                lifetime,
                mutability,
                inner,
            } => ir::Ref(
                self.lower_region(*lifetime, params),
                self.lower_ty(inner, params),
                Self::lower_mutability(*mutability),
            ),
            Stored::RawPointer { mutability, inner } => ir::RawPtr(
                self.lower_ty(inner, params),
                Self::lower_mutability(*mutability),
            ),
            Stored::Adt(adt) => ir::Adt(
                self.adt_def(DefId::Adt(adt.def)),
                self.lower_args(&adt.args, params),
            ),
            Stored::Param(p) => match self.param(GenericParamRef::Type(*p), params) {
                Some(p) => ir::Param(p),
                None => ir::Error(ErrorGuaranteed),
            },
            Stored::Alias(crate::AliasTy::Projection(p)) => ir::Alias(ir::AliasTy::new_from_args(
                self,
                ir::AliasTyKind::Projection {
                    def_id: DefId::TypeAlias(p.associated_ty),
                },
                self.complete_args(
                    DefId::TypeAlias(p.associated_ty),
                    self.lower_args(&p.args, params),
                ),
            )),
            Stored::Alias(crate::AliasTy::Opaque(o)) => ir::Alias(ir::AliasTy::new_from_args(
                self,
                ir::AliasTyKind::Opaque {
                    def_id: DefId::Opaque(o.opaque),
                },
                self.complete_args(DefId::Opaque(o.opaque), self.lower_args(&o.args, params)),
            )),
            Stored::FnDef(f) => ir::FnDef(DefId::Function(f.def), self.lower_args(&f.args, params)),
            Stored::FnPointer {
                params: inputs,
                ret,
            } => return self.lower_fn_pointer(inputs, ret, params, false),
            Stored::Closure(c) => {
                // Capture classification is outside this engine's scope. Preserve the existing
                // callable model; the solver still sees the closure's real input/output variables.
                let kind = Ty::from_closure_kind(self, ir::ClosureKind::Fn);
                let sig = self.lower_fn_pointer(&c.params, &c.ret, params, true);
                let captures = Ty::new_unit(self);
                ir::Closure(
                    DefId::Closure(c.id),
                    List::new(self, &[kind.into(), sig.into(), captures.into()]),
                )
            }
            Stored::Unknown => ir::Error(ErrorGuaranteed),
        };
        Ty::new(self, kind)
    }

    fn lower_fn_pointer(
        self,
        inputs: &[crate::Ty],
        ret: &crate::Ty,
        params: &[GenericParamRef],
        tuple_inputs: bool,
    ) -> Ty<'s> {
        let mut tys = inputs
            .iter()
            .map(|t| self.lower_ty(t, params))
            .collect::<Vec<_>>();
        if tuple_inputs {
            tys = vec![Ty::new_tup(self, &tys)];
        }
        tys.push(self.lower_ty(ret, params));
        Ty::new_fn_ptr(
            self,
            ir::Binder::dummy(ir::FnSig {
                inputs_and_output: List::new(self, &tys),
                fn_sig_kind: ir::FnSigKind::new(
                    if tuple_inputs {
                        rustc_abi::ExternAbi::RustCall
                    } else {
                        rustc_abi::ExternAbi::Rust
                    },
                    Safety(true),
                    false,
                ),
            }),
        )
    }

    pub fn lower_args(
        self,
        args: &crate::GenericArgs,
        params: &[GenericParamRef],
    ) -> GenericArgs<'s> {
        let args = args
            .iter()
            .map(|arg| match arg {
                crate::GenericArg::Type(ty) => self.lower_ty(ty, params).into(),
                crate::GenericArg::Lifetime(r) => self.lower_region(*r, params).into(),
                crate::GenericArg::Const(c) => self.lower_const(*c, params).into(),
            })
            .collect::<Vec<_>>();
        List::new(self, &args)
    }

    pub(crate) fn lower_region(self, r: crate::Lifetime, params: &[GenericParamRef]) -> Region<'s> {
        Region(match r {
            crate::Lifetime::Static => ir::ReStatic,
            crate::Lifetime::Erased => ir::ReErased,
            crate::Lifetime::Param(p) => match self.param(GenericParamRef::Lifetime(p), params) {
                Some(p) => ir::ReEarlyParam(p),
                None => ir::ReError(ErrorGuaranteed),
            },
        })
    }

    pub fn lower_const(self, c: crate::ConstValue, params: &[GenericParamRef]) -> Const<'s> {
        Const::new(
            self,
            match c {
                crate::ConstValue::Scalar(value) => ir::ConstKind::Value(ValueConst {
                    ty: Ty::new_usize(self),
                    value,
                }),
                crate::ConstValue::Param(p) => {
                    match self.param(GenericParamRef::Const(p), params) {
                        Some(p) => ir::ConstKind::Param(p),
                        None => ir::ConstKind::Error(ErrorGuaranteed),
                    }
                }
                crate::ConstValue::Unknown => ir::ConstKind::Error(ErrorGuaranteed),
            },
        )
    }

    pub(crate) fn lower_trait_ref(
        self,
        tr: &crate::TraitApplication,
        params: &[GenericParamRef],
    ) -> ir::TraitRef<Self> {
        ir::TraitRef::new_from_args(
            self,
            DefId::Trait(tr.def),
            self.complete_args(DefId::Trait(tr.def), self.lower_args(&tr.args, params)),
        )
    }

    pub fn lower_clause(self, clause: &crate::Clause, params: &[GenericParamRef]) -> Clause<'s> {
        match clause {
            crate::Clause::Implemented(tr) => self.lower_trait_ref(tr, params).upcast(self),
            crate::Clause::AliasEq { alias, ty } => ir::ProjectionPredicate {
                projection_term: ir::AliasTerm::new_from_args(
                    self,
                    ir::AliasTermKind::ProjectionTy {
                        def_id: DefId::TypeAlias(alias.associated_ty),
                    },
                    self.complete_args(
                        DefId::TypeAlias(alias.associated_ty),
                        self.lower_args(&alias.args, params),
                    ),
                ),
                term: self.lower_ty(ty, params).into(),
            }
            .upcast(self),
        }
    }

    /// Declaration lowering emits only trait and associated-type equality clauses. Exporting
    /// them lets another operation reuse the template without retaining this arena.
    pub(crate) fn raise_clause(self, clause: Clause<'s>) -> crate::Clause {
        match clause.kind().skip_binder() {
            ir::ClauseKind::Trait(tr) => {
                let DefId::Trait(def) = tr.trait_ref.def_id else {
                    unreachable!("source trait clause has a trait identity");
                };
                crate::Clause::Implemented(crate::TraitApplication {
                    def,
                    args: self.raise_args(tr.trait_ref.args),
                })
            }
            ir::ClauseKind::Projection(projection) => {
                let DefId::TypeAlias(associated_ty) = projection.projection_term.def_id() else {
                    unreachable!("source projection has an associated type identity");
                };
                crate::Clause::AliasEq {
                    alias: crate::ProjectionTy {
                        associated_ty,
                        args: self.raise_args(projection.projection_term.args),
                    },
                    ty: self
                        .raise_ty(projection.term.expect_ty())
                        .unwrap_or(crate::Ty::Unknown),
                }
            }
            _ => unreachable!("source lowering emits trait and projection clauses"),
        }
    }

    /// Export stable shapes. Unresolved components become `Unknown`; declaration parameters
    /// keep their identities, and inference-variable IDs never enter the owned result.
    pub fn raise_ty(self, ty: Ty<'s>) -> Option<crate::Ty> {
        use crate::Ty as Stored;
        Some(match ty.kind() {
            ir::Bool => Stored::Primitive(crate::PrimitiveTy::Bool),
            ir::Char => Stored::Primitive(crate::PrimitiveTy::Char),
            ir::Str => Stored::Primitive(crate::PrimitiveTy::Str),
            ir::Int(i) => Stored::Primitive(crate::PrimitiveTy::from_name(i.name_str())?),
            ir::Uint(i) => Stored::Primitive(crate::PrimitiveTy::from_name(i.name_str())?),
            ir::Float(f) => Stored::Primitive(crate::PrimitiveTy::from_name(f.name_str())?),
            ir::Never => Stored::Never,
            ir::Tuple(fields) => Stored::tuple(
                fields
                    .iter()
                    .map(|t| self.raise_ty(t).unwrap_or(Stored::Unknown))
                    .collect(),
            ),
            ir::Array(t, c) => Stored::array(
                self.raise_ty(t).unwrap_or(Stored::Unknown),
                self.raise_const(c),
            ),
            ir::Slice(t) => Stored::slice(self.raise_ty(t).unwrap_or(Stored::Unknown)),
            ir::Ref(r, t, m) => Stored::Reference {
                lifetime: Self::raise_region(r),
                mutability: Self::raise_mutability(m),
                inner: Box::new(self.raise_ty(t).unwrap_or(Stored::Unknown)),
            },
            ir::RawPtr(t, m) => Stored::raw_pointer(
                Self::raise_mutability(m),
                self.raise_ty(t).unwrap_or(Stored::Unknown),
            ),
            ir::Adt(adt, args) => {
                let DefId::Adt(def) = adt.def_id() else {
                    return None;
                };
                Stored::adt(crate::AdtTy {
                    def,
                    args: self.raise_args(args),
                })
            }
            ir::Param(Param {
                source: GenericParamRef::Type(p),
                ..
            }) => Stored::Param(p),
            ir::Alias(alias) => match alias.kind {
                ir::AliasTyKind::Projection {
                    def_id: DefId::TypeAlias(associated_ty),
                } => Stored::Alias(crate::AliasTy::Projection(crate::ProjectionTy {
                    associated_ty,
                    args: self.raise_args(alias.args),
                })),
                ir::AliasTyKind::Opaque {
                    def_id: DefId::Opaque(opaque),
                } => Stored::Alias(crate::AliasTy::Opaque(crate::OpaqueTy {
                    opaque,
                    args: self.raise_args(alias.args),
                })),
                _ => return None,
            },
            ir::FnDef(DefId::Function(def), args) => {
                Stored::fn_def_with_args(def, self.raise_args(args))
            }
            ir::FnPtr(sig, _) => {
                let sig = sig.skip_binder();
                let inputs = sig
                    .inputs()
                    .iter()
                    .map(|t| self.raise_ty(t).unwrap_or(Stored::Unknown))
                    .collect();
                Stored::fn_pointer(
                    inputs,
                    self.raise_ty(sig.output()).unwrap_or(Stored::Unknown),
                )
            }
            ir::Closure(DefId::Closure(id), args) => {
                let sig = args.as_closure().sig().skip_binder();
                let inputs = sig
                    .inputs()
                    .first()?
                    .tuple_fields()
                    .iter()
                    .map(|t| self.raise_ty(t).unwrap_or(Stored::Unknown))
                    .collect();
                Stored::closure(
                    id,
                    inputs,
                    self.raise_ty(sig.output()).unwrap_or(Stored::Unknown),
                )
            }
            _ => return None,
        })
    }

    pub fn raise_args(self, args: GenericArgs<'s>) -> crate::GenericArgs {
        args.iter()
            .map(|arg| match arg.kind() {
                ir::GenericArgKind::Type(t) => crate::GenericArg::Type(Box::new(
                    self.raise_ty(t).unwrap_or(crate::Ty::Unknown),
                )),
                ir::GenericArgKind::Lifetime(r) => {
                    crate::GenericArg::Lifetime(Self::raise_region(r))
                }
                ir::GenericArgKind::Const(c) => crate::GenericArg::Const(self.raise_const(c)),
            })
            .collect()
    }

    fn raise_region(r: Region<'s>) -> crate::Lifetime {
        match r.kind() {
            ir::ReStatic => crate::Lifetime::Static,
            ir::ReEarlyParam(Param {
                source: GenericParamRef::Lifetime(p),
                ..
            }) => crate::Lifetime::Param(p),
            _ => crate::Lifetime::Erased,
        }
    }

    pub fn raise_const(self, c: Const<'s>) -> crate::ConstValue {
        match c.kind() {
            ir::ConstKind::Value(v) => crate::ConstValue::Scalar(v.value),
            ir::ConstKind::Param(Param {
                source: GenericParamRef::Const(p),
                ..
            }) => crate::ConstValue::Param(p),
            _ => crate::ConstValue::Unknown,
        }
    }

    pub(crate) fn lower_mutability(m: crate::Mutability) -> ir::Mutability {
        match m {
            crate::Mutability::Shared => ir::Mutability::Not,
            crate::Mutability::Mutable => ir::Mutability::Mut,
        }
    }

    pub(crate) fn raise_mutability(m: ir::Mutability) -> crate::Mutability {
        match m {
            ir::Mutability::Not => crate::Mutability::Shared,
            ir::Mutability::Mut => crate::Mutability::Mutable,
        }
    }

    pub(crate) fn lower_primitive(self, p: crate::PrimitiveTy) -> Ty<'s> {
        let kind = match p {
            crate::PrimitiveTy::Bool => ir::Bool,
            crate::PrimitiveTy::Char => ir::Char,
            crate::PrimitiveTy::Str => ir::Str,
            crate::PrimitiveTy::SignedInt(i) => ir::Int(match i {
                crate::SignedIntTy::I8 => ir::IntTy::I8,
                crate::SignedIntTy::I16 => ir::IntTy::I16,
                crate::SignedIntTy::I32 => ir::IntTy::I32,
                crate::SignedIntTy::I64 => ir::IntTy::I64,
                crate::SignedIntTy::I128 => ir::IntTy::I128,
                crate::SignedIntTy::Isize => ir::IntTy::Isize,
            }),
            crate::PrimitiveTy::UnsignedInt(i) => ir::Uint(match i {
                crate::UnsignedIntTy::U8 => ir::UintTy::U8,
                crate::UnsignedIntTy::U16 => ir::UintTy::U16,
                crate::UnsignedIntTy::U32 => ir::UintTy::U32,
                crate::UnsignedIntTy::U64 => ir::UintTy::U64,
                crate::UnsignedIntTy::U128 => ir::UintTy::U128,
                crate::UnsignedIntTy::Usize => ir::UintTy::Usize,
            }),
            crate::PrimitiveTy::Float(f) => ir::Float(match f {
                crate::FloatTy::F32 => ir::FloatTy::F32,
                crate::FloatTy::F64 => ir::FloatTy::F64,
            }),
        };
        Ty::new(self, kind)
    }
}
