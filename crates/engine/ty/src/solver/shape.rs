//! Glancer-facing views and constructors for live solver types.
//!
//! These views contain only copied arena handles. They do not own another type tree or variable
//! table, and inspecting a shape does not normalize it or choose fallback types.

use rustc_type_ir::{
    self as ir, TypeVisitableExt,
    inherent::{GenericArgs as _, IntoKind, Ty as _},
};

use super::{
    Const, DefId, GenericArgs, List, Region, SolverInterner, Ty,
    types::{ErrorGuaranteed, Safety},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferVarKind {
    Type,
    Integer,
    Float,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdtTy<'s> {
    pub def: rg_ir_model::TypeDefRef,
    pub args: GenericArgs<'s>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FnDefTy<'s> {
    pub def: rg_ir_model::FunctionRef,
    pub args: GenericArgs<'s>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClosureTy<'s> {
    pub id: crate::ClosureTyId,
    pub params: List<'s, Ty<'s>>,
    pub ret: Ty<'s>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectionTy<'s> {
    pub associated_ty: rg_ir_model::TypeAliasRef,
    pub args: GenericArgs<'s>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpaqueTy<'s> {
    pub opaque: rg_ir_model::OpaqueTyRef,
    pub args: GenericArgs<'s>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasTy<'s> {
    Projection(ProjectionTy<'s>),
    Opaque(OpaqueTy<'s>),
}

/// The outer structure body inference can inspect without using compiler-specific type variants.
///
/// `Vec<?T>` exposes an ADT and its live arguments. A variable already assigned to `Vec<?T>`
/// still exposes `InferVar` here: callers resolve its root through the table before inspecting
/// it. Nested variables remain live, so reading a field or tuple element can preserve them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TyShape<'s> {
    Unit,
    Never,
    Primitive(crate::PrimitiveTy),
    Tuple(List<'s, Ty<'s>>),
    Array {
        inner: Ty<'s>,
        len: Const<'s>,
    },
    Slice(Ty<'s>),
    Reference {
        lifetime: Region<'s>,
        mutability: crate::Mutability,
        inner: Ty<'s>,
    },
    RawPointer {
        mutability: crate::Mutability,
        inner: Ty<'s>,
    },
    FnPointer {
        params: List<'s, Ty<'s>>,
        ret: Ty<'s>,
    },
    Adt(AdtTy<'s>),
    Param(rg_ir_model::TypeParamRef),
    Alias(AliasTy<'s>),
    Closure(ClosureTy<'s>),
    FnDef(FnDefTy<'s>),
    Unknown,
    InferVar {
        kind: InferVarKind,
    },
}

impl<'s> Ty<'s> {
    pub fn shape(self) -> TyShape<'s> {
        use TyShape as S;
        match self.kind() {
            ir::Never => S::Never,
            ir::Tuple(fields) if fields.is_empty() => S::Unit,
            ir::Tuple(fields) => S::Tuple(fields),
            ir::Bool => S::Primitive(crate::PrimitiveTy::Bool),
            ir::Char => S::Primitive(crate::PrimitiveTy::Char),
            ir::Str => S::Primitive(crate::PrimitiveTy::Str),
            ir::Int(p) => {
                S::Primitive(crate::PrimitiveTy::from_name(p.name_str()).expect("integer spelling"))
            }
            ir::Uint(p) => {
                S::Primitive(crate::PrimitiveTy::from_name(p.name_str()).expect("integer spelling"))
            }
            ir::Float(p) => {
                crate::PrimitiveTy::from_name(p.name_str()).map_or(S::Unknown, S::Primitive)
            }
            ir::Array(inner, len) => S::Array { inner, len },
            ir::Slice(inner) => S::Slice(inner),
            ir::Ref(lifetime, inner, mutability) => S::Reference {
                lifetime,
                inner,
                mutability: SolverInterner::raise_mutability(mutability),
            },
            ir::RawPtr(inner, mutability) => S::RawPointer {
                inner,
                mutability: SolverInterner::raise_mutability(mutability),
            },
            ir::Adt(adt, args) => match adt.id {
                DefId::Adt(def) => S::Adt(AdtTy { def, args }),
                _ => S::Unknown,
            },
            ir::FnDef(DefId::Function(def), args) => S::FnDef(FnDefTy { def, args }),
            ir::FnPtr(sig, _) => S::FnPointer {
                params: sig.skip_binder().inputs(),
                ret: sig.skip_binder().output(),
            },
            ir::Closure(DefId::Closure(id), args) => {
                let sig = args.as_closure().sig().skip_binder();
                S::Closure(ClosureTy {
                    id,
                    params: sig.inputs()[0].tuple_fields(),
                    ret: sig.output(),
                })
            }
            ir::Param(super::Param {
                source: rg_ir_model::GenericParamRef::Type(p),
                ..
            }) => S::Param(p),
            ir::Alias(alias) => match alias.kind {
                ir::AliasTyKind::Projection {
                    def_id: DefId::TypeAlias(associated_ty),
                } => S::Alias(AliasTy::Projection(ProjectionTy {
                    associated_ty,
                    args: alias.args,
                })),
                ir::AliasTyKind::Opaque {
                    def_id: DefId::Opaque(opaque),
                } => S::Alias(AliasTy::Opaque(OpaqueTy {
                    opaque,
                    args: alias.args,
                })),
                _ => S::Unknown,
            },
            ir::Infer(ir::TyVar(_)) => S::InferVar {
                kind: InferVarKind::Type,
            },
            ir::Infer(ir::IntVar(_)) => S::InferVar {
                kind: InferVarKind::Integer,
            },
            ir::Infer(ir::FloatVar(_)) => S::InferVar {
                kind: InferVarKind::Float,
            },
            _ => S::Unknown,
        }
    }

    pub fn reference_inner(self) -> Option<(Self, crate::Mutability)> {
        match self.shape() {
            TyShape::Reference {
                inner, mutability, ..
            } => Some((inner, mutability)),
            _ => None,
        }
    }

    pub fn has_projection(self) -> bool {
        self.has_type_flags(ir::TypeFlags::HAS_TY_PROJECTION)
    }

    pub fn as_adt(self) -> Option<AdtTy<'s>> {
        match self.shape() {
            TyShape::Adt(adt) => Some(adt),
            _ => None,
        }
    }
}

impl<'s> SolverInterner<'s> {
    pub fn unknown(self) -> Ty<'s> {
        Ty::new_error(self, ErrorGuaranteed)
    }

    pub fn unit(self) -> Ty<'s> {
        Ty::new_unit(self)
    }

    pub fn never(self) -> Ty<'s> {
        Ty::new(self, ir::Never)
    }

    pub fn primitive(self, p: crate::PrimitiveTy) -> Ty<'s> {
        self.lower_primitive(p)
    }

    pub fn tuple(self, fields: impl AsRef<[Ty<'s>]>) -> Ty<'s> {
        Ty::new_tup(self, fields.as_ref())
    }

    pub fn array(self, inner: Ty<'s>, len: Const<'s>) -> Ty<'s> {
        Ty::new_array_with_const_len(self, inner, len)
    }

    pub fn scalar(self, value: u128) -> Const<'s> {
        Const::new(
            self,
            ir::ConstKind::Value(super::types::ValueConst {
                ty: Ty::new_usize(self),
                value,
            }),
        )
    }

    pub fn slice(self, inner: Ty<'s>) -> Ty<'s> {
        Ty::new_slice(self, inner)
    }

    pub fn reference(self, mutability: crate::Mutability, inner: Ty<'s>) -> Ty<'s> {
        self.reference_with_lifetime(Region(ir::ReErased), mutability, inner)
    }

    pub fn reference_with_lifetime(
        self,
        region: Region<'s>,
        mutability: crate::Mutability,
        inner: Ty<'s>,
    ) -> Ty<'s> {
        Ty::new_ref(self, region, inner, Self::lower_mutability(mutability))
    }

    pub fn raw_pointer(self, mutability: crate::Mutability, inner: Ty<'s>) -> Ty<'s> {
        Ty::new_ptr(self, inner, Self::lower_mutability(mutability))
    }

    pub fn adt(self, adt: AdtTy<'s>) -> Ty<'s> {
        use ir::Interner;
        Ty::new_adt(self, self.adt_def(DefId::Adt(adt.def)), adt.args)
    }

    pub fn projection(self, projection: ProjectionTy<'s>) -> Ty<'s> {
        Ty::new(
            self,
            ir::Alias(ir::AliasTy::new_from_args(
                self,
                ir::AliasTyKind::Projection {
                    def_id: DefId::TypeAlias(projection.associated_ty),
                },
                self.complete_args(DefId::TypeAlias(projection.associated_ty), projection.args),
            )),
        )
    }

    pub fn fn_def(self, function: rg_ir_model::FunctionRef, args: GenericArgs<'s>) -> Ty<'s> {
        Ty::new(self, ir::FnDef(DefId::Function(function), args))
    }

    pub fn fn_pointer(self, params: &[Ty<'s>], ret: Ty<'s>) -> Ty<'s> {
        self.function_pointer(params, ret, rustc_abi::ExternAbi::Rust)
    }

    fn function_pointer(self, params: &[Ty<'s>], ret: Ty<'s>, abi: rustc_abi::ExternAbi) -> Ty<'s> {
        let tys = params.iter().copied().chain([ret]).collect::<Vec<_>>();
        Ty::new_fn_ptr(
            self,
            ir::Binder::dummy(ir::FnSig {
                inputs_and_output: List::new(self, &tys),
                fn_sig_kind: ir::FnSigKind::new(abi, Safety(true), false),
            }),
        )
    }

    pub fn closure(self, id: crate::ClosureTyId, params: &[Ty<'s>], ret: Ty<'s>) -> Ty<'s> {
        let sig = self.function_pointer(&[self.tuple(params)], ret, rustc_abi::ExternAbi::RustCall);
        Ty::new_closure(
            self,
            DefId::Closure(id),
            List::new(
                self,
                &[
                    Ty::from_closure_kind(self, ir::ClosureKind::Fn).into(),
                    sig.into(),
                    self.unit().into(),
                ],
            ),
        )
    }
}

impl<'s> super::GenericArg<'s> {
    pub fn as_ty(self) -> Option<Ty<'s>> {
        match self.0 {
            ir::GenericArgKind::Type(ty) => Some(ty),
            _ => None,
        }
    }
}
