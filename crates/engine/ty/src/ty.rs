//! Owned semantic types that can outlive a type operation.
//!
//! Item declarations keep source-shaped `TypeRef` values; lowering and inference use scoped
//! `solver::Ty` values. Saved facts, shared declaration templates, and independent query results
//! use these owned shapes, which carry declaration identities but no inference-table state.

use std::fmt;

use rg_ir_model::{
    BodyRef, ExprId, FunctionRef, OpaqueTyRef, TraitDefRef, TypeAliasRef, TypeDefRef, TypeParamRef,
};
use rg_semantic_ir::TypePathResolution;
use rg_std::{ExpectedUnique, MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::{ConstValue, GenericArg, GenericArgs, Lifetime, Mutability, PrimitiveTy};

/// Identity of one anonymous closure type.
///
/// Expression indices are only unique inside one body. The body identity is therefore part of the
/// type identity when a closure enters a type query; otherwise two bodies whose
/// first closure is `e0` could be mistaken for the same closure.
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
/// The scoped solver keeps signature components connected during inference, then publishes this
/// owned signature with every remaining variable finalized.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ClosureTy {
    pub id: ClosureTyId,
    #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<Ty>>")]
    pub params: Vec<Ty>,
    #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty>>")]
    pub ret: Box<Ty>,
}

/// Owned types for saved facts, independent query results, and shared declaration templates.
///
/// Inference variables live only in `solver::Ty`. A body exports its learned assignments into
/// these types at completion; an unanswered variable becomes `Unknown`. Declaration parameters
/// and projections can remain, because their identities do not depend on an inference table.
/// Known structure surrounds unknown components: `&Missing` keeps its reference, with `Unknown`
/// only in the referent. Mutability and lifetimes still carry useful information in that case.
///
/// Names are resolved before a type is stored here. A generic parameter remembers where it was
/// declared, and `Self` inside `impl dyn Service` is stored as `dyn Service`.
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
    Dyn(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<DynTy>>")] Box<DynTy>),
    Param(TypeParamRef),
    Alias(AliasTy),
    Closure(ClosureTy),
    // Function definition types remain distinct from function pointers. The argument list is part
    // of identity even when it consists entirely of unknown or inferred positions.
    FnDef(FnDefTy),
    Unknown,
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

    /// Visit this type and each inner type behind a written `&T` or `&mut T`.
    /// Patterns and type-definition queries only need these wrappers; member lookup also needs
    /// trait `Deref` and uses the inference table's receiver walk instead.
    pub fn reference_chain(&self) -> impl Iterator<Item = &Self> {
        const MAX_REFERENCE_PEELING_DEPTH: usize = 8;
        std::iter::successors(Some(self), |ty| {
            ty.reference_inner().map(|(inner, _)| inner)
        })
        .take(MAX_REFERENCE_PEELING_DEPTH + 1)
    }

    pub fn reference_inner(&self) -> Option<(&Self, Mutability)> {
        match self {
            Self::Reference {
                mutability, inner, ..
            } => Some((inner, *mutability)),
            _ => None,
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
            Self::Dyn(ty) => ty.types().any(Self::has_unknown),
            Self::Alias(alias) => alias.has_unknown(),
            Self::Closure(closure) => {
                closure.params.iter().any(Self::has_unknown) || closure.ret.has_unknown()
            }
            Self::FnDef(function) => function.args.iter().any(GenericArg::has_unknown),
            Self::Unknown => true,
            Self::Unit | Self::Never | Self::Primitive(_) | Self::Param(_) => false,
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
            Self::Dyn(ty) => ty.types().any(Self::has_projection),
            Self::Alias(AliasTy::Opaque(alias)) => {
                alias.args.iter().any(GenericArg::has_projection)
            }
            Self::Closure(closure) => {
                closure.params.iter().any(Self::has_projection) || closure.ret.has_projection()
            }
            Self::FnDef(function) => function.args.iter().any(GenericArg::has_projection),
            Self::Unit | Self::Never | Self::Primitive(_) | Self::Param(_) | Self::Unknown => false,
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
            Self::Dyn(ty) => ty.types().any(Self::has_closure),
            Self::Alias(alias) => alias.args().iter().any(GenericArg::has_closure),
            Self::FnDef(function) => function.args.iter().any(GenericArg::has_closure),
            Self::Unit | Self::Never | Self::Primitive(_) | Self::Param(_) | Self::Unknown => false,
        }
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
            Self::Dyn(ty) => Shrink::shrink_to_fit(ty),
            Self::Alias(alias) => Shrink::shrink_to_fit(alias),
            Self::Closure(closure) => Shrink::shrink_to_fit(closure),
            Self::FnDef(function) => Shrink::shrink_to_fit(function),
            Self::Unit | Self::Never | Self::Primitive(_) | Self::Param(_) | Self::Unknown => {}
        }
    }
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

/// A trait object type that can be kept after inference finishes.
///
/// For `dyn Service<Request, Output = Reply>`, `def` identifies `Service`, `args` contains
/// `[Request]`, and `associated_types` contains the `Output = Reply` binding.
///
/// In `Worker: Service<Request>`, `Worker` fills the trait's `Self` argument. A `dyn` type
/// does not name the type implementing the trait, so we leave `Self` out of its arguments.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct DynTy {
    pub def: TraitDefRef,
    pub args: GenericArgs,
    #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<DynAssocTypeBinding>>")]
    pub associated_types: Vec<DynAssocTypeBinding>,
    pub lifetime: Lifetime,
}

impl DynTy {
    /// Return the child types for recursive checks, such as looking for an unknown type.
    /// For `dyn Service<Request, Output = Reply>`, this includes `Request` and `Reply`.
    /// Bindings inherited from another trait can have their own arguments; include those too.
    pub fn types(&self) -> impl Iterator<Item = &Ty> {
        self.args
            .iter()
            .filter_map(GenericArg::as_ty)
            .chain(self.associated_types.iter().flat_map(|binding| {
                binding
                    .args
                    .iter()
                    .filter_map(GenericArg::as_ty)
                    .chain([&binding.ty])
            }))
    }
}

/// The `Item = u8` part of a type like `dyn Iterator<Item = u8>`.
///
/// `associated_ty` points to `Iterator::Item`, and `ty` holds `u8`. `args` holds the generic
/// arguments of the trait that declares `Item`, with no entry for the hidden `Self` type.
///
/// These arguments can differ from the ones next to `dyn`: if `Derived<T>: Base<Vec<T>>`
/// and `Base` declares `Item`, then `dyn Derived<T, Item = u8>` needs `[Vec<T>]` here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct DynAssocTypeBinding {
    pub associated_ty: TypeAliasRef,
    pub args: GenericArgs,
    pub ty: Ty,
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

impl AliasTy {
    pub(crate) fn args(&self) -> &GenericArgs {
        match self {
            Self::Projection(alias) => &alias.args,
            Self::Opaque(alias) => &alias.args,
        }
    }

    fn has_unknown(&self) -> bool {
        self.args().iter().any(GenericArg::has_unknown)
    }
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
