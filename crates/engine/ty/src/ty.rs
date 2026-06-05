use rg_ir_model::items::{GenericParams, TypeRef};
use rg_ir_model::{TypeDefRef, TypePathResolution};
use rg_memsize::Shrink;
use rg_text::Name;

use crate::{GenericArg, PrimitiveTy, RefMutability};

/// Ordered substitutions for type parameters visible at one use site.
///
/// Substitutions are intentionally stack-like: later bindings shadow earlier bindings. Body
/// resolution extends this set while walking through aliases, impl headers, and function
/// signatures, so lookup must search from the end instead of treating the data as an unordered map.
#[derive(Debug, Clone, Default, PartialEq, Eq, rg_memsize::MemorySize)]
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
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub enum Ty {
    Unit,
    Never,
    Primitive(PrimitiveTy),
    Reference {
        mutability: RefMutability,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty>>")]
        inner: Box<Ty>,
    },
    Syntax(TypeRef),
    Nominal(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<NominalTy>>")] Vec<NominalTy>),
    SelfTy(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<NominalTy>>")] Vec<NominalTy>),
    Unknown,
}

/// Module-level nominal type together with the generic arguments visible at use site.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct NominalTy {
    pub def: TypeDefRef,
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

    fn shrink_to_fit(&mut self) {
        self.args.shrink_to_fit();
        for arg in &mut self.args {
            arg.shrink_to_fit();
        }
    }
}

impl Ty {
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

    pub fn nominal(types: Vec<NominalTy>) -> Self {
        Self::Nominal(types)
    }

    pub fn self_ty(types: Vec<NominalTy>) -> Self {
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
            Self::Nominal(types) | Self::SelfTy(types) => types,
            Self::Unit
            | Self::Never
            | Self::Primitive(_)
            | Self::Reference { .. }
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
            | Self::Syntax(_)
            | Self::Nominal(_)
            | Self::SelfTy(_)
            | Self::Unknown => None,
        }
    }

    pub fn one_or_unknown(mut tys: Vec<Self>) -> Self {
        if tys.len() == 1 {
            tys.pop().expect("one type should exist")
        } else {
            Self::Unknown
        }
    }

    pub fn shrink_to_fit(&mut self) {
        match self {
            Self::Reference { inner, .. } => inner.shrink_to_fit(),
            Self::Syntax(ty) => ty.shrink_to_fit(),
            Self::Nominal(types) | Self::SelfTy(types) => {
                types.shrink_to_fit();
                for ty in types {
                    ty.shrink_to_fit();
                }
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
