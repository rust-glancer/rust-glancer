use rg_ir_model::items::{GenericParams, TypeRef};
use rg_ir_model::{TraitRef, TypeDefRef, TypePathResolution};
use rg_std::{MemorySize, Shrink, UniqueVec};
use rg_text::Name;

use crate::{GenericArg, PrimitiveTy, RefMutability};
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
        mutability: RefMutability,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty>>")]
        inner: Box<Ty>,
    },
    Opaque {
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<UniqueVec<OpaqueTraitBound>>")]
        bounds: UniqueVec<OpaqueTraitBound>,
    },
    Syntax(TypeRef),
    Nominal(
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<UniqueVec<NominalTy>>")]
        UniqueVec<NominalTy>,
    ),
    SelfTy(
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<UniqueVec<NominalTy>>")]
        UniqueVec<NominalTy>,
    ),
    Unknown,
}

/// Module-level nominal type together with the generic arguments visible at use site.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct NominalTy {
    #[shrink(skip)]
    pub def: TypeDefRef,
    #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<GenericArg>>")]
    pub args: Vec<GenericArg>,
}

/// Resolved trait bound preserved for opaque `impl Trait` types.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct OpaqueTraitBound {
    #[shrink(skip)]
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

    pub fn reference(mutability: RefMutability, inner: Self) -> Self {
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

    pub fn nominal(types: UniqueVec<NominalTy>) -> Self {
        Self::Nominal(types)
    }

    pub fn self_ty(types: UniqueVec<NominalTy>) -> Self {
        if types.is_empty() {
            return Self::Unknown;
        }

        Self::SelfTy(types)
    }

    /// Projects a resolved nominal type path into `Ty`, preserving source generic arguments.
    ///
    /// Aliases, traits, and unresolved paths need context-specific fallback behavior, so callers
    /// decide how to handle them.
    pub fn from_type_path_resolution(
        resolution: TypePathResolution,
        args: Vec<GenericArg>,
    ) -> Option<Self> {
        // Attach the generic arguments from the source path to whichever nominal definition the
        // path resolved to. Ambiguous multi-target resolution keeps the same args on every
        // candidate.
        match resolution {
            TypePathResolution::SelfType(types) => Some(Self::self_ty(
                types
                    .into_iter()
                    .map(|def| NominalTy {
                        def,
                        args: args.clone(),
                    })
                    .collect(),
            )),
            TypePathResolution::TypeDefs(types) => Some(Self::nominal(
                types
                    .into_iter()
                    .map(|def| NominalTy {
                        def,
                        args: args.clone(),
                    })
                    .collect(),
            )),
            TypePathResolution::TypeAliases(_)
            | TypePathResolution::Traits(_)
            | TypePathResolution::Unknown => None,
        }
    }

    pub fn as_nominals(&self) -> &[NominalTy] {
        match self {
            Self::Nominal(types) | Self::SelfTy(types) => types.as_slice(),
            Self::Unit
            | Self::Never
            | Self::Primitive(_)
            | Self::Tuple(_)
            | Self::Array { .. }
            | Self::Slice(_)
            | Self::Reference { .. }
            | Self::Opaque { .. }
            | Self::Syntax(_)
            | Self::Unknown => &[],
        }
    }

    pub fn reference_inner(&self) -> Option<(&Self, RefMutability)> {
        match self {
            Self::Reference { mutability, inner } => Some((inner, *mutability)),
            Self::Unit
            | Self::Never
            | Self::Primitive(_)
            | Self::Tuple(_)
            | Self::Array { .. }
            | Self::Slice(_)
            | Self::Opaque { .. }
            | Self::Syntax(_)
            | Self::Nominal(_)
            | Self::SelfTy(_)
            | Self::Unknown => None,
        }
    }

    pub fn one_or_unknown(tys: UniqueVec<Self>) -> Self {
        let mut vec = tys.into_vec();
        if vec.len() == 1 {
            vec.pop().unwrap()
        } else {
            Self::Unknown
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
            Self::Unit | Self::Never | Self::Primitive(_) | Self::Nominal(_) | Self::SelfTy(_) => {
                true
            }
        }
    }

    pub fn shrink_to_fit(&mut self) {
        match self {
            Self::Tuple(fields) => {
                fields.shrink_to_fit();
                for field in fields {
                    field.shrink_to_fit();
                }
            }
            Self::Array { inner, len } => {
                inner.shrink_to_fit();
                if let Some(len) = len {
                    len.shrink_to_fit();
                }
            }
            Self::Slice(inner) => inner.shrink_to_fit(),
            Self::Reference { inner, .. } => inner.shrink_to_fit(),
            Self::Opaque { bounds } => {
                Shrink::shrink_to_fit(bounds);
            }
            Self::Syntax(ty) => ty.shrink_to_fit(),
            Self::Nominal(types) | Self::SelfTy(types) => {
                Shrink::shrink_to_fit(types);
            }
            Self::Unit | Self::Never | Self::Primitive(_) | Self::Unknown => {}
        }
    }
}

impl Shrink for Ty {
    fn shrink_to_fit(&mut self) {
        Ty::shrink_to_fit(self);
    }
}
