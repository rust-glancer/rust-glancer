//! Canonical semantic type shapes.
//!
//! Item declarations keep source-shaped `TypeRef` values for display and navigation. After the
//! lowering boundary, type algorithms use only the identities and full argument lists in this
//! module; they do not compare source text or reinterpret declaration syntax.

use std::fmt;

use rg_ir_model::{
    BodyRef, ExprId, FunctionRef, OpaqueTyRef, TypeAliasRef, TypeDefRef, TypeParamRef,
};
use rg_semantic_ir::TypePathResolution;
use rg_std::{ExpectedUnique, MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::inference::{InferVarId, InferVarKind};
use crate::{ConstValue, GenericArg, GenericArgs, Lifetime, Mutability, PrimitiveTy};

/// Identity of one anonymous closure type.
///
/// Expression indices are only unique inside one body. The body identity is therefore part of the
/// type identity before a closure enters the crate-scoped trait solver; otherwise two bodies whose
/// first closure is `e0` could reuse the same cached Chalk answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ClosureTyId {
    body: BodyRef,
    expr: ExprId,
}

impl ClosureTyId {
    pub fn new(body: BodyRef, expr: ExprId) -> Self {
        Self { body, expr }
    }
}

impl fmt::Display for ClosureTyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.expr.0.fmt(f)
    }
}

/// Anonymous closure type together with the callable signature inferred for that expression.
///
/// The signature types may contain body inference variables. Keeping them in the closure type is
/// intentional: expected `Fn*` bounds, the closure patterns/body, and Chalk all constrain the same
/// slots instead of exchanging a separate body-only witness.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ClosureTy {
    pub id: ClosureTyId,
    #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<Ty>>")]
    pub params: Vec<Ty>,
    #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty>>")]
    pub ret: Box<Ty>,
}

/// Owned semantic types shared by indexing and body analysis.
///
/// Every identity-carrying variant is self-contained: syntax text is not an equality key, generic
/// parameters carry their owner, and inherent `Self` is the same `Adt` as its concrete spelling.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub enum Ty {
    Unit,
    Never,
    Primitive(PrimitiveTy),
    Tuple(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<Ty>>")] Vec<Ty>),
    Array {
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty>>")]
        inner: Box<Ty>,
        len: ConstValue,
    },
    Slice(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty>>")] Box<Ty>),
    Reference {
        lifetime: Lifetime,
        mutability: Mutability,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty>>")]
        inner: Box<Ty>,
    },
    RawPointer {
        mutability: Mutability,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty>>")]
        inner: Box<Ty>,
    },
    FnPointer {
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<Ty>>")]
        params: Vec<Ty>,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty>>")]
        ret: Box<Ty>,
    },
    Adt(AdtTy),
    Param(TypeParamRef),
    Alias(AliasTy),
    Closure(ClosureTy),
    // Function definition types remain distinct from function pointers. The argument list is part
    // of identity even when it consists entirely of unknown or inferred positions.
    FnDef(FnDefTy),
    Unknown,
    /// Transient inference variable. It must be finalized before persistence.
    InferVar {
        #[wincode(with = "rg_wincode_utils::WincodeUnsupported<InferVarKind>")]
        kind: InferVarKind,
        #[wincode(with = "rg_wincode_utils::WincodeUnsupported<InferVarId>")]
        id: InferVarId,
    },
}

/// Algebraic data type together with its full semantic argument list.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct AdtTy {
    pub def: TypeDefRef,
    pub args: GenericArgs,
}

impl AdtTy {
    pub fn bare(def: TypeDefRef) -> Self {
        Self {
            def,
            args: GenericArgs::empty(),
        }
    }
}

/// Instantiated type of one function definition.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct FnDefTy {
    pub def: FunctionRef,
    pub args: GenericArgs,
}

/// Semantic alias identities that are not transparent type aliases.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum AliasTy {
    Projection(ProjectionTy),
    Opaque(OpaqueTy),
}

/// Associated type selected from a fully instantiated trait application.
///
/// For `<Vec<User> as IntoIterator>::Item`, `associated_ty` identifies the `Item` declaration and
/// `args` retains `Self = Vec<User>` plus every declared `IntoIterator` argument. The value of the
/// projection is resolved separately by trait selection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ProjectionTy {
    pub associated_ty: TypeAliasRef,
    pub args: GenericArgs,
}

impl ProjectionTy {
    /// Compare projections after bijectively renaming transient inference-variable IDs.
    pub(crate) fn equivalent_modulo_inference_ids(&self, other: &Self) -> bool {
        self.associated_ty == other.associated_ty
            && self.args.equivalent_modulo_inference_ids(&other.args)
    }
}

/// One opaque `impl Trait` occurrence instantiated with its owner's generic arguments.
///
/// In `fn make<T>() -> impl Iterator<Item = T>`, `opaque` identifies this particular `impl Trait`
/// occurrence and `args` records the chosen `T`. Its `Iterator` predicates are queryable signature
/// data rather than part of opaque type equality.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct OpaqueTy {
    pub opaque: OpaqueTyRef,
    pub args: GenericArgs,
}

impl Ty {
    pub fn tuple(fields: Vec<Self>) -> Self {
        if fields.is_empty() {
            Self::Unit
        } else {
            Self::Tuple(fields)
        }
    }

    pub fn array(inner: Self, len: impl Into<ConstValue>) -> Self {
        Self::Array {
            inner: Box::new(inner),
            len: len.into(),
        }
    }

    pub fn slice(inner: Self) -> Self {
        Self::Slice(Box::new(inner))
    }

    pub fn reference(mutability: Mutability, inner: Self) -> Self {
        Self::reference_with_lifetime(Lifetime::Erased, mutability, inner)
    }

    pub fn reference_with_lifetime(
        lifetime: Lifetime,
        mutability: Mutability,
        inner: Self,
    ) -> Self {
        if matches!(inner, Self::Unknown) {
            return Self::Unknown;
        }

        Self::Reference {
            lifetime,
            mutability,
            inner: Box::new(inner),
        }
    }

    pub fn raw_pointer(mutability: Mutability, inner: Self) -> Self {
        Self::RawPointer {
            mutability,
            inner: Box::new(inner),
        }
    }

    pub fn fn_pointer(params: Vec<Self>, ret: Self) -> Self {
        Self::FnPointer {
            params,
            ret: Box::new(ret),
        }
    }

    pub fn closure(id: ClosureTyId, params: Vec<Self>, ret: Self) -> Self {
        Self::Closure(ClosureTy {
            id,
            params,
            ret: Box::new(ret),
        })
    }

    pub fn fn_def_with_args(function: FunctionRef, args: impl Into<GenericArgs>) -> Self {
        Self::FnDef(FnDefTy {
            def: function,
            args: args.into(),
        })
    }

    pub fn adt(ty: AdtTy) -> Self {
        Self::Adt(ty)
    }

    pub(crate) fn var_for_kind(kind: InferVarKind, id: InferVarId) -> Self {
        Self::InferVar { kind, id }
    }

    /// Projects the identity result of a path lookup into a semantic type.
    ///
    /// Transparent aliases require recursive lowering and traits are not types, so those cases are
    /// handled by the central lowerer rather than this identity-only helper.
    pub fn from_type_path_resolution(
        resolution: TypePathResolution,
        args: impl Into<GenericArgs>,
    ) -> Option<Self> {
        let args = args.into();
        match resolution {
            TypePathResolution::SelfType(def) | TypePathResolution::TypeDef(def) => {
                Some(Self::adt(AdtTy { def, args }))
            }
            TypePathResolution::TypeAlias(_)
            | TypePathResolution::Trait(_)
            | TypePathResolution::Unknown => None,
        }
    }

    pub fn as_adts(&self) -> &[AdtTy] {
        match self {
            Self::Adt(ty) => std::slice::from_ref(ty),
            _ => &[],
        }
    }

    /// Return whether this type has a concrete impl head without a nominal lookup key.
    ///
    /// These receiver shapes cannot use the `TypeDefRef` index, so member lookup must consider the
    /// compact structural and blanket-impl fallback lists. `User` and `Vec<User>` are nominal and
    /// return false; `str`, `[User]`, `[User; 3]`, and `*const User` return true. A generic `T` also
    /// has no key, but it is not a concrete receiver shape and therefore returns false here.
    pub fn has_unkeyed_self_head(&self) -> bool {
        matches!(
            self,
            Self::Unit
                | Self::Never
                | Self::Primitive(_)
                | Self::Tuple(_)
                | Self::Array { .. }
                | Self::Slice(_)
                | Self::Reference { .. }
                | Self::RawPointer { .. }
                | Self::FnPointer { .. }
                | Self::Closure(_)
                | Self::FnDef(_)
        )
    }

    pub fn reference_inner(&self) -> Option<(&Self, Mutability)> {
        match self {
            Self::Reference {
                mutability, inner, ..
            } => Some((inner, *mutability)),
            _ => None,
        }
    }

    pub fn has_var(&self) -> bool {
        match self {
            Self::InferVar { .. } => true,
            Self::Tuple(fields) => fields.iter().any(Self::has_var),
            Self::Array { inner, .. }
            | Self::Slice(inner)
            | Self::Reference { inner, .. }
            | Self::RawPointer { inner, .. } => inner.has_var(),
            Self::FnPointer { params, ret } => params.iter().any(Self::has_var) || ret.has_var(),
            Self::Adt(ty) => ty.args.iter().any(GenericArg::has_var),
            Self::Alias(alias) => alias.has_var(),
            Self::Closure(closure) => {
                closure.params.iter().any(Self::has_var) || closure.ret.has_var()
            }
            Self::FnDef(function) => function.args.iter().any(GenericArg::has_var),
            Self::Unit | Self::Never | Self::Primitive(_) | Self::Param(_) | Self::Unknown => false,
        }
    }

    pub fn has_unknown(&self) -> bool {
        match self {
            Self::Tuple(fields) => fields.iter().any(Self::has_unknown),
            Self::Array { inner, .. }
            | Self::Slice(inner)
            | Self::Reference { inner, .. }
            | Self::RawPointer { inner, .. } => inner.has_unknown(),
            Self::FnPointer { params, ret } => {
                params.iter().any(Self::has_unknown) || ret.has_unknown()
            }
            Self::Adt(ty) => ty.args.iter().any(GenericArg::has_unknown),
            Self::Alias(alias) => alias.has_unknown(),
            Self::Closure(closure) => {
                closure.params.iter().any(Self::has_unknown) || closure.ret.has_unknown()
            }
            Self::FnDef(function) => function.args.iter().any(GenericArg::has_unknown),
            Self::Unknown => true,
            Self::Unit
            | Self::Never
            | Self::Primitive(_)
            | Self::Param(_)
            | Self::InferVar { .. } => false,
        }
    }

    /// Return whether normalization still has an associated-type projection to resolve.
    pub fn has_projection(&self) -> bool {
        match self {
            Self::Alias(AliasTy::Projection(_)) => true,
            Self::Tuple(fields) => fields.iter().any(Self::has_projection),
            Self::Array { inner, .. }
            | Self::Slice(inner)
            | Self::Reference { inner, .. }
            | Self::RawPointer { inner, .. } => inner.has_projection(),
            Self::FnPointer { params, ret } => {
                params.iter().any(Self::has_projection) || ret.has_projection()
            }
            Self::Adt(ty) => ty.args.iter().any(GenericArg::has_projection),
            Self::Alias(AliasTy::Opaque(alias)) => {
                alias.args.iter().any(GenericArg::has_projection)
            }
            Self::Closure(closure) => {
                closure.params.iter().any(Self::has_projection) || closure.ret.has_projection()
            }
            Self::FnDef(function) => function.args.iter().any(GenericArg::has_projection),
            Self::Unit
            | Self::Never
            | Self::Primitive(_)
            | Self::Param(_)
            | Self::Unknown
            | Self::InferVar { .. } => false,
        }
    }

    /// Return whether the type contains an anonymous closure type.
    pub fn has_closure(&self) -> bool {
        match self {
            Self::Closure(_) => true,
            Self::Tuple(fields) => fields.iter().any(Self::has_closure),
            Self::Array { inner, .. }
            | Self::Slice(inner)
            | Self::Reference { inner, .. }
            | Self::RawPointer { inner, .. } => inner.has_closure(),
            Self::FnPointer { params, ret } => {
                params.iter().any(Self::has_closure) || ret.has_closure()
            }
            Self::Adt(ty) => ty.args.iter().any(GenericArg::has_closure),
            Self::Alias(alias) => alias.args().iter().any(GenericArg::has_closure),
            Self::FnDef(function) => function.args.iter().any(GenericArg::has_closure),
            Self::Unit
            | Self::Never
            | Self::Primitive(_)
            | Self::Param(_)
            | Self::Unknown
            | Self::InferVar { .. } => false,
        }
    }

    pub(crate) fn is_projectable(&self) -> bool {
        match self {
            Self::Unknown | Self::InferVar { .. } => false,
            Self::Tuple(fields) => fields.iter().all(Self::is_projectable),
            Self::Array { inner, .. }
            | Self::Slice(inner)
            | Self::Reference { inner, .. }
            | Self::RawPointer { inner, .. } => inner.is_projectable(),
            Self::FnPointer { params, ret } => {
                params.iter().all(Self::is_projectable) && ret.is_projectable()
            }
            Self::Adt(ty) => ty.args.iter().all(GenericArg::is_projectable),
            Self::Alias(alias) => alias.is_projectable(),
            Self::Closure(closure) => {
                closure.params.iter().all(Self::is_projectable) && closure.ret.is_projectable()
            }
            Self::FnDef(function) => function.args.iter().all(GenericArg::is_projectable),
            Self::Unit | Self::Never | Self::Primitive(_) | Self::Param(_) => true,
        }
    }
}

impl AliasTy {
    pub(crate) fn args(&self) -> &GenericArgs {
        match self {
            Self::Projection(alias) => &alias.args,
            Self::Opaque(alias) => &alias.args,
        }
    }

    pub(crate) fn same_definition(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Projection(lhs), Self::Projection(rhs)) => {
                lhs.associated_ty == rhs.associated_ty
            }
            (Self::Opaque(lhs), Self::Opaque(rhs)) => lhs.opaque == rhs.opaque,
            (Self::Projection(_), Self::Opaque(_)) | (Self::Opaque(_), Self::Projection(_)) => {
                false
            }
        }
    }

    fn has_var(&self) -> bool {
        self.args().iter().any(GenericArg::has_var)
    }

    fn has_unknown(&self) -> bool {
        self.args().iter().any(GenericArg::has_unknown)
    }

    fn is_projectable(&self) -> bool {
        self.args().iter().all(GenericArg::is_projectable)
    }
}

/// Converts expected-unique type candidates into the public type vocabulary.
pub trait ExpectedTyExt {
    fn into_ty(self) -> Ty;
}

impl ExpectedTyExt for ExpectedUnique<Ty> {
    fn into_ty(self) -> Ty {
        self.into_option().unwrap_or(Ty::Unknown)
    }
}

/// Converts expected-unique ADT candidates into the public type vocabulary.
pub trait ExpectedAdtTyExt {
    fn into_adt_ty(self) -> Ty;
}

impl ExpectedAdtTyExt for ExpectedUnique<AdtTy> {
    fn into_adt_ty(self) -> Ty {
        self.into_option().map(Ty::adt).unwrap_or(Ty::Unknown)
    }
}

impl Shrink for Ty {
    fn shrink_to_fit(&mut self) {
        match self {
            Self::Tuple(fields) => Shrink::shrink_to_fit(fields),
            Self::Array { inner, .. }
            | Self::Slice(inner)
            | Self::Reference { inner, .. }
            | Self::RawPointer { inner, .. } => Shrink::shrink_to_fit(inner),
            Self::FnPointer { params, ret } => {
                Shrink::shrink_to_fit(params);
                Shrink::shrink_to_fit(ret);
            }
            Self::Adt(ty) => Shrink::shrink_to_fit(ty),
            Self::Alias(alias) => Shrink::shrink_to_fit(alias),
            Self::Closure(closure) => Shrink::shrink_to_fit(closure),
            Self::FnDef(function) => Shrink::shrink_to_fit(function),
            Self::Unit
            | Self::Never
            | Self::Primitive(_)
            | Self::Param(_)
            | Self::Unknown
            | Self::InferVar { .. } => {}
        }
    }
}
