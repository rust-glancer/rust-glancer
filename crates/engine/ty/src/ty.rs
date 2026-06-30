use std::fmt;

use rg_ir_model::items::{GenericParams, TypeRef};
use rg_ir_model::{ExprId, TraitRef, TypeDefRef, TypePathResolution};
use rg_std::{ExpectedUnique, MemorySize, Shrink, UniqueVec};
use rg_text::Name;

use crate::{GenericArg, Mutability, PrimitiveTy};
use wincode::{SchemaRead, SchemaWrite};

/// Ordered substitutions for type parameters visible at one use site.
///
/// Substitutions are intentionally stack-like: later bindings shadow earlier bindings. Body
/// resolution extends this set while walking through aliases, impl headers, and function
/// signatures, so lookup must search from the end instead of treating the data as an unordered map.
#[derive(Debug, Clone, Default, PartialEq, Eq, MemorySize)]
pub struct TypeSubst(Vec<(Name, Ty)>);

impl TypeSubst {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds direct type-parameter substitutions from declared generics and visible arguments.
    pub fn from_generics(generics: &GenericParams, args: &[GenericArg]) -> Self {
        // We only substitute type parameters. Lifetimes, const args, associated type args, and
        // unsupported args are preserved on the type but ignored by the simple substitution map.
        let type_args = args.iter().filter_map(|arg| arg.as_ty().cloned());

        generics
            .types
            .iter()
            .zip(type_args)
            .map(|(param, ty)| (param.name.clone(), ty))
            .collect()
    }

    /// Adds a binding after existing entries, making it the visible one for later lookups.
    pub fn push(&mut self, name: Name, ty: Ty) {
        self.0.push((name, ty));
    }

    /// Appends another substitution set, preserving its internal shadowing order.
    pub fn extend(&mut self, subst: Self) {
        self.0.extend(subst.0);
    }

    /// Returns the visible binding for `name`, honoring later shadowing earlier entries.
    pub fn get(&self, name: &str) -> Option<&Ty> {
        self.0
            .iter()
            .rev()
            .find_map(|(param, ty)| (param.as_str() == name).then_some(ty))
    }

    /// Returns the visible substitution for a plain type-parameter name.
    pub fn type_param(&self, name: &str) -> Option<Ty> {
        self.get(name).cloned()
    }
}

impl FromIterator<(Name, Ty)> for TypeSubst {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = (Name, Ty)>,
    {
        Self(iter.into_iter().collect())
    }
}

/// Body-local identity of an anonymous closure type.
///
/// Rust gives every closure expression its own anonymous type. This id preserves that identity
/// inside one body without pretending that it is a stable cross-body item.
///
/// Example: the closure at `ExprId(12)` can become `ClosureTyId::new(ExprId(12))`, so body
/// inference can later find that closure's params and body result. The id does not encode
/// captures or which callable trait the closure implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ClosureTyId(ExprId);

impl ClosureTyId {
    pub fn new(expr: ExprId) -> Self {
        Self(expr)
    }

    pub fn into_expr_id(self) -> ExprId {
        self.0
    }
}

impl fmt::Display for ClosureTyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.0.fmt(f)
    }
}

/// Small type vocabulary shared by IR layers.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub enum Ty {
    Unit,
    Never,
    Primitive(PrimitiveTy),
    Tuple(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<Ty>>")] Vec<Ty>),
    Array {
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty>>")]
        inner: Box<Ty>,
        len: Option<String>,
    },
    Slice(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty>>")] Box<Ty>),
    Reference {
        mutability: Mutability,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty>>")]
        inner: Box<Ty>,
    },
    Opaque {
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<UniqueVec<OpaqueTraitBound>>")]
        bounds: UniqueVec<OpaqueTraitBound>,
    },
    Closure(ClosureTyId),
    Syntax(TypeRef),
    Nominal(NominalTy),
    SelfTy(NominalTy),
    Unknown,
}

/// Module-level nominal type together with the generic arguments visible at use site.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct NominalTy {
    pub def: TypeDefRef,
    #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<GenericArg>>")]
    pub args: Vec<GenericArg>,
}

/// Resolved trait bound preserved for opaque `impl Trait` types.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct OpaqueTraitBound {
    pub trait_ref: TraitRef,
    #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<GenericArg>>")]
    pub args: Vec<GenericArg>,
}

impl NominalTy {
    pub fn bare(def: TypeDefRef) -> Self {
        Self {
            def,
            args: Vec::new(),
        }
    }
}

impl Ty {
    pub fn tuple(fields: Vec<Self>) -> Self {
        if fields.is_empty() {
            return Self::Unit;
        }

        Self::Tuple(fields)
    }

    pub fn array(inner: Self, len: Option<String>) -> Self {
        Self::Array {
            inner: Box::new(inner),
            len,
        }
    }

    pub fn slice(inner: Self) -> Self {
        Self::Slice(Box::new(inner))
    }

    pub fn reference(mutability: Mutability, inner: Self) -> Self {
        if matches!(inner, Self::Unknown) {
            return Self::Unknown;
        }

        Self::Reference {
            mutability,
            inner: Box::new(inner),
        }
    }

    pub fn syntax(ty: TypeRef) -> Self {
        Self::Syntax(ty)
    }

    pub fn opaque(bounds: UniqueVec<OpaqueTraitBound>) -> Self {
        if bounds.is_empty() {
            return Self::Unknown;
        }

        Self::Opaque { bounds }
    }

    pub fn closure(id: ClosureTyId) -> Self {
        Self::Closure(id)
    }

    pub fn nominal(ty: NominalTy) -> Self {
        Self::Nominal(ty)
    }

    pub fn self_ty(ty: NominalTy) -> Self {
        Self::SelfTy(ty)
    }

    /// Projects a resolved nominal type path into `Ty`, preserving source generic arguments.
    ///
    /// Aliases, traits, and unresolved paths need context-specific fallback behavior, so callers
    /// decide how to handle them.
    pub fn from_type_path_resolution(
        resolution: TypePathResolution,
        args: Vec<GenericArg>,
    ) -> Option<Self> {
        match resolution {
            TypePathResolution::SelfType(def) => Some(Self::self_ty(NominalTy { def, args })),
            TypePathResolution::TypeDef(def) => Some(Self::nominal(NominalTy { def, args })),
            TypePathResolution::TypeAlias(_)
            | TypePathResolution::Trait(_)
            | TypePathResolution::Unknown => None,
        }
    }

    pub fn as_nominals(&self) -> &[NominalTy] {
        match self {
            Self::Nominal(ty) | Self::SelfTy(ty) => std::slice::from_ref(ty),
            Self::Unit
            | Self::Never
            | Self::Primitive(_)
            | Self::Tuple(_)
            | Self::Array { .. }
            | Self::Slice(_)
            | Self::Reference { .. }
            | Self::Opaque { .. }
            | Self::Closure(_)
            | Self::Syntax(_)
            | Self::Unknown => &[],
        }
    }

    pub fn reference_inner(&self) -> Option<(&Self, Mutability)> {
        match self {
            Self::Reference { mutability, inner } => Some((inner, *mutability)),
            Self::Unit
            | Self::Never
            | Self::Primitive(_)
            | Self::Tuple(_)
            | Self::Array { .. }
            | Self::Slice(_)
            | Self::Opaque { .. }
            | Self::Closure(_)
            | Self::Syntax(_)
            | Self::Nominal(_)
            | Self::SelfTy(_)
            | Self::Unknown => None,
        }
    }

    /// Returns true when this type shape contains `Ty::Unknown`.
    pub fn has_unknown(&self) -> bool {
        match self {
            Self::Tuple(fields) => fields.iter().any(Self::has_unknown),
            Self::Array { inner, .. } | Self::Slice(inner) | Self::Reference { inner, .. } => {
                inner.has_unknown()
            }
            Self::Opaque { bounds } => bounds
                .iter()
                .any(|bound| bound.args.iter().any(GenericArg::has_unknown)),
            Self::Nominal(ty) | Self::SelfTy(ty) => ty.args.iter().any(GenericArg::has_unknown),
            Self::Unknown => true,
            Self::Unit | Self::Never | Self::Primitive(_) | Self::Closure(_) | Self::Syntax(_) => {
                false
            }
        }
    }

    pub(crate) fn is_projectable(&self) -> bool {
        match self {
            Self::Unknown | Self::Syntax(_) => false,
            Self::Tuple(fields) => fields.iter().all(Self::is_projectable),
            Self::Array { inner, .. } | Self::Slice(inner) => inner.is_projectable(),
            Self::Reference { inner, .. } => inner.is_projectable(),
            Self::Opaque { bounds } => bounds
                .iter()
                .all(|bound| bound.args.iter().all(GenericArg::is_projectable)),
            Self::Unit
            | Self::Never
            | Self::Primitive(_)
            | Self::Closure(_)
            | Self::Nominal(_)
            | Self::SelfTy(_) => true,
        }
    }
}

/// Converts expected-unique type candidates into the public type vocabulary.
pub trait ExpectedTyExt {
    /// Converts the value to `Ty` or keeps it as `Ty::Unknown`.
    fn into_ty(self) -> Ty;
}

impl ExpectedTyExt for ExpectedUnique<Ty> {
    fn into_ty(self) -> Ty {
        self.into_option().unwrap_or(Ty::Unknown)
    }
}

/// Converts expected-unique nominal candidates into the public type vocabulary.
pub trait ExpectedNominalTyExt {
    /// Converts the value to `Ty::Nominal` or keeps it as `Ty::Unknown`.
    fn into_nominal_ty(self) -> Ty;

    /// Converts the value to `Ty::SelfTy` or keeps it as `Ty::Unknown`.
    fn into_self_ty(self) -> Ty;
}

impl ExpectedNominalTyExt for ExpectedUnique<NominalTy> {
    fn into_nominal_ty(self) -> Ty {
        self.into_option().map(Ty::nominal).unwrap_or(Ty::Unknown)
    }

    fn into_self_ty(self) -> Ty {
        self.into_option().map(Ty::self_ty).unwrap_or(Ty::Unknown)
    }
}

impl Shrink for Ty {
    fn shrink_to_fit(&mut self) {
        match self {
            Self::Tuple(fields) => {
                Shrink::shrink_to_fit(fields);
            }
            Self::Array { inner, len } => {
                Shrink::shrink_to_fit(inner);
                Shrink::shrink_to_fit(len);
            }
            Self::Slice(inner) => Shrink::shrink_to_fit(inner),
            Self::Reference { inner, .. } => Shrink::shrink_to_fit(inner),
            Self::Opaque { bounds } => {
                Shrink::shrink_to_fit(bounds);
            }
            Self::Syntax(ty) => Shrink::shrink_to_fit(ty),
            Self::Nominal(ty) | Self::SelfTy(ty) => {
                Shrink::shrink_to_fit(ty);
            }
            Self::Unit | Self::Never | Self::Primitive(_) | Self::Closure(_) | Self::Unknown => {}
        }
    }
}
