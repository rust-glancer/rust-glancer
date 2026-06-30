use std::fmt;

use wincode::{SchemaRead, SchemaWrite};

use rg_parse::Span;
use rg_std::{MemorySize, Shrink};
use rg_text::Name;

use crate::Mutability;

/// Unresolved type syntax lowered into the item tree.
///
/// This intentionally stops before semantic resolution. `TypeRef` represents what the user wrote
/// in an item declaration; resolving paths to definitions belongs to later IR layers.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum TypeRef {
    Unknown(String),
    Never,
    Unit,
    Infer,
    Path(#[wincode(with = "rg_wincode_utils::WincodeDynamic<TypePath>")] TypePath),
    Tuple(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<TypeRef>>")] Vec<TypeRef>),
    Reference {
        lifetime: Option<String>,
        mutability: Mutability,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<TypeRef>>")]
        inner: Box<TypeRef>,
    },
    RawPointer {
        mutability: Mutability,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<TypeRef>>")]
        inner: Box<TypeRef>,
    },
    Slice(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<TypeRef>>")] Box<TypeRef>),
    Array {
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<TypeRef>>")]
        inner: Box<TypeRef>,
        len: Option<String>,
    },
    FnPointer {
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<TypeRef>>")]
        params: Vec<TypeRef>,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<TypeRef>>")]
        ret: Box<TypeRef>,
    },
    ImplTrait(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<TypeBound>>")] Vec<TypeBound>),
    DynTrait(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<TypeBound>>")] Vec<TypeBound>),
}

impl TypeRef {
    pub fn unknown_from_text(text: impl Into<String>) -> Self {
        Self::Unknown(text.into())
    }

    /// Returns true when this type syntax is the special `Self` type.
    pub fn is_self_type(&self) -> bool {
        matches!(self, Self::Path(path) if path.is_self_type())
    }

    /// Returns the name of a plain single-segment type path.
    pub fn type_param_name(&self) -> Option<Name> {
        match self {
            Self::Path(path) => path.single_name().cloned(),
            _ => None,
        }
    }

    /// Returns the simple associated path shape `T::Assoc`.
    ///
    /// This is syntax-only: for `S::Item`, this returns `S` and `Item`, but the caller still has
    /// to decide whether `S` is actually one of the relevant type parameters.
    pub fn as_type_param_assoc_path(&self) -> Option<(&Name, &Name)> {
        if let Self::Path(path) = self
            && !path.absolute
            && let [param_segment, assoc_segment] = path.segments.as_slice()
            && param_segment.args.is_empty()
            && assoc_segment.args.is_empty()
        {
            return Some((&param_segment.name, &assoc_segment.name));
        }

        None
    }

    /// Returns true when this type syntax contains explicit generic arguments anywhere inside it.
    pub fn has_generic_args(&self) -> bool {
        match self {
            Self::Path(path) => path.segments.iter().any(|segment| !segment.args.is_empty()),
            Self::Tuple(types) => types.iter().any(Self::has_generic_args),
            Self::Reference { inner, .. }
            | Self::RawPointer { inner, .. }
            | Self::Slice(inner)
            | Self::Array { inner, .. } => inner.has_generic_args(),
            Self::FnPointer { params, ret } => {
                params.iter().any(Self::has_generic_args) || ret.has_generic_args()
            }
            Self::ImplTrait(bounds) | Self::DynTrait(bounds) => {
                bounds.iter().any(TypeBound::has_generic_args)
            }
            Self::Unknown(_) | Self::Never | Self::Unit | Self::Infer => false,
        }
    }

    /// Returns true when this type syntax mentions one of the provided type parameter names.
    pub fn mentions_type_param(&self, params: &[&str]) -> bool {
        match self {
            Self::Path(path) => path.segments.iter().any(|segment| {
                params.contains(&segment.name.as_str())
                    || segment
                        .args
                        .iter()
                        .any(|arg| arg.mentions_type_param(params))
            }),
            Self::Tuple(types) => types.iter().any(|ty| ty.mentions_type_param(params)),
            Self::Reference { inner, .. }
            | Self::RawPointer { inner, .. }
            | Self::Slice(inner)
            | Self::Array { inner, .. } => inner.mentions_type_param(params),
            Self::FnPointer {
                params: fn_params,
                ret,
            } => {
                fn_params.iter().any(|ty| ty.mentions_type_param(params))
                    || ret.mentions_type_param(params)
            }
            Self::ImplTrait(bounds) | Self::DynTrait(bounds) => {
                bounds.iter().any(|bound| bound.mentions_type_param(params))
            }
            Self::Unknown(_) | Self::Never | Self::Unit | Self::Infer => false,
        }
    }
}

impl fmt::Display for TypeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(text) if text.is_empty() => write!(f, "<unknown>"),
            Self::Unknown(text) => write!(f, "<unsupported:{text}>"),
            Self::Never => write!(f, "!"),
            Self::Unit => write!(f, "()"),
            Self::Infer => write!(f, "_"),
            Self::Path(path) => write!(f, "{path}"),
            Self::Tuple(types) => {
                write!(f, "(")?;
                for (idx, ty) in types.iter().enumerate() {
                    if idx > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{ty}")?;
                }
                if types.len() == 1 {
                    write!(f, ",")?;
                }
                write!(f, ")")
            }
            Self::Reference {
                lifetime,
                mutability,
                inner,
            } => {
                write!(f, "&")?;
                if let Some(lifetime) = lifetime {
                    write!(f, "{lifetime} ")?;
                }
                if matches!(mutability, Mutability::Mutable) {
                    write!(f, "mut ")?;
                }
                write!(f, "{inner}")
            }
            Self::RawPointer { mutability, inner } => match mutability {
                Mutability::Shared => write!(f, "*const {inner}"),
                Mutability::Mutable => write!(f, "*mut {inner}"),
            },
            Self::Slice(inner) => write!(f, "[{inner}]"),
            Self::Array { inner, len } => {
                write!(f, "[{inner}; ")?;
                match len {
                    Some(len) => write!(f, "{len}")?,
                    None => write!(f, "<unknown>")?,
                }
                write!(f, "]")
            }
            Self::FnPointer { params, ret } => {
                write!(f, "fn(")?;
                for (idx, param) in params.iter().enumerate() {
                    if idx > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{param}")?;
                }
                write!(f, ")")?;
                if !matches!(ret.as_ref(), TypeRef::Unit) {
                    write!(f, " -> {ret}")?;
                }
                Ok(())
            }
            Self::ImplTrait(bounds) => write_bounds(f, "impl ", bounds),
            Self::DynTrait(bounds) => write_bounds(f, "dyn ", bounds),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct TypePath {
    /// Full source range of the path syntax, including separators around segments.
    #[shrink(skip)]
    pub source_span: Span,
    pub absolute: bool,
    #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<TypePathSegment>>")]
    pub segments: Vec<TypePathSegment>,
}

impl TypePath {
    /// Returns the name of a single-segment relative path.
    pub fn single_name(&self) -> Option<&Name> {
        if self.absolute || self.segments.len() != 1 {
            return None;
        }

        self.segments.first().map(|segment| &segment.name)
    }

    pub fn is_self_type(&self) -> bool {
        self.single_name()
            .is_some_and(|name| name.as_str() == "Self")
    }
}

impl fmt::Display for TypePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.absolute {
            write!(f, "::")?;
        }

        for (idx, segment) in self.segments.iter().enumerate() {
            if idx > 0 {
                write!(f, "::")?;
            }
            write!(f, "{segment}")?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct TypePathSegment {
    pub name: Name,
    #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<GenericArg>>")]
    pub args: Vec<GenericArg>,
    #[shrink(skip)]
    pub span: Span,
}

impl fmt::Display for TypePathSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)?;
        if let [GenericArg::FnTraitArgs { params, ret }] = self.args.as_slice() {
            write_fn_trait_args(f, params, ret)?;
            return Ok(());
        }

        if !self.args.is_empty() {
            write!(f, "<")?;
            for (idx, arg) in self.args.iter().enumerate() {
                if idx > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{arg}")?;
            }
            write!(f, ">")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum GenericArg {
    Type(#[wincode(with = "rg_wincode_utils::WincodeDynamic<TypeRef>")] TypeRef),
    Lifetime(String),
    Const(String),
    /// Parenthesized argument syntax on function-trait paths, such as `FnOnce(T) -> R`.
    FnTraitArgs {
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<TypeRef>>")]
        params: Vec<TypeRef>,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<TypeRef>>")]
        ret: Box<TypeRef>,
    },
    AssocType {
        name: Name,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Option<TypeRef>>")]
        ty: Option<TypeRef>,
    },
    Unsupported(String),
}

impl GenericArg {
    /// Extracts the syntax type from this argument when it is a type argument.
    pub fn type_ref(&self) -> Option<&TypeRef> {
        match self {
            Self::Type(ty) => Some(ty),
            Self::Lifetime(_)
            | Self::Const(_)
            | Self::FnTraitArgs { .. }
            | Self::AssocType { .. }
            | Self::Unsupported(_) => None,
        }
    }

    /// Returns true when this generic argument mentions one of the provided type parameter names.
    pub fn mentions_type_param(&self, params: &[&str]) -> bool {
        match self {
            Self::Type(ty) => ty.mentions_type_param(params),
            Self::AssocType { ty, .. } => {
                ty.as_ref().is_some_and(|ty| ty.mentions_type_param(params))
            }
            Self::FnTraitArgs {
                params: fn_params,
                ret,
            } => {
                fn_params.iter().any(|ty| ty.mentions_type_param(params))
                    || ret.mentions_type_param(params)
            }
            Self::Lifetime(_) | Self::Const(_) | Self::Unsupported(_) => false,
        }
    }
}

impl fmt::Display for GenericArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Type(ty) => write!(f, "{ty}"),
            Self::Lifetime(lifetime) => write!(f, "{lifetime}"),
            Self::Const(value) => write!(f, "{value}"),
            Self::AssocType { name, ty } => match ty {
                Some(ty) => write!(f, "{name} = {ty}"),
                None => write!(f, "{name}"),
            },
            Self::FnTraitArgs { params, ret } => write_fn_trait_args(f, params, ret),
            Self::Unsupported(text) => write!(f, "<unsupported:{text}>"),
        }
    }
}

fn write_fn_trait_args(
    f: &mut fmt::Formatter<'_>,
    params: &[TypeRef],
    ret: &TypeRef,
) -> fmt::Result {
    write!(f, "(")?;
    for (idx, param) in params.iter().enumerate() {
        if idx > 0 {
            write!(f, ", ")?;
        }
        write!(f, "{param}")?;
    }
    write!(f, ")")?;
    if !matches!(ret, TypeRef::Unit) {
        write!(f, " -> {ret}")?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum TypeBound {
    Trait(#[wincode(with = "rg_wincode_utils::WincodeDynamic<TypeRef>")] TypeRef),
    Lifetime(String),
    Unsupported(String),
}

impl TypeBound {
    /// Returns true when this bound contains explicit generic arguments anywhere inside it.
    pub fn has_generic_args(&self) -> bool {
        match self {
            Self::Trait(ty) => ty.has_generic_args(),
            Self::Lifetime(_) | Self::Unsupported(_) => false,
        }
    }

    /// Returns true when this bound mentions one of the provided type parameter names.
    pub fn mentions_type_param(&self, params: &[&str]) -> bool {
        match self {
            Self::Trait(ty) => ty.mentions_type_param(params),
            Self::Lifetime(_) | Self::Unsupported(_) => false,
        }
    }
}

impl fmt::Display for TypeBound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trait(ty) => write!(f, "{ty}"),
            Self::Lifetime(lifetime) => write!(f, "{lifetime}"),
            Self::Unsupported(text) => write!(f, "<unsupported:{text}>"),
        }
    }
}

fn write_bounds(f: &mut fmt::Formatter<'_>, prefix: &str, bounds: &[TypeBound]) -> fmt::Result {
    write!(f, "{prefix}")?;
    for (idx, bound) in bounds.iter().enumerate() {
        if idx > 0 {
            write!(f, " + ")?;
        }
        write!(f, "{bound}")?;
    }
    Ok(())
}
