use std::fmt;

use wincode::{SchemaRead, SchemaWrite};

use rg_parse::Span;
use rg_std::{MemorySize, Shrink};
use rg_text::Name;

use rg_ir_model::{Mutability, Path, PathSegment};

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
        lifetime: Option<Name>,
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

    /// Projects a plain path type into the compact form used by definition lookup.
    ///
    /// Anchored associated paths carry more information than a DefMap path and therefore return
    /// `None` instead of silently dropping their anchor.
    pub fn as_def_map_path(&self) -> Option<Path> {
        let Self::Path(path) = self else {
            return None;
        };
        path.as_def_map_path()
    }

    /// Returns the simple associated path shape `T::Assoc`.
    ///
    /// This is syntax-only: for `S::Item`, this returns `S` and `Item`, but the caller still has
    /// to decide whether `S` is actually one of the relevant type parameters.
    pub fn as_type_param_assoc_path(&self) -> Option<(&Name, &Name)> {
        if let Self::Path(path) = self
            && path.anchor.is_none()
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
            Self::Path(path) => path.has_generic_args(),
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
            Self::Path(path) => path.mentions_type_param(params),
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

    /// Displays this type through one caller-provided semantic-name policy.
    ///
    /// The item tree owns the recursive type-syntax traversal. Presentation layers only decide how
    /// canonical names are spelled, such as whether the use-site edition requires `r#`.
    pub fn display_with<F>(&self, names: F) -> TypeRefDisplay<'_, F>
    where
        F: TypeNameFormatter,
    {
        TypeRefDisplay { ty: self, names }
    }
}

impl fmt::Display for TypeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        TypeFormatter::canonical().fmt_type_ref(self, f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct TypePath {
    /// Full source range of the path syntax, including separators around segments.
    #[shrink(skip)]
    pub source_span: Span,
    pub absolute: bool,
    #[wincode(with = "rg_wincode_utils::WincodeDynamic<Option<TypePathAnchor>>")]
    pub anchor: Option<TypePathAnchor>,
    #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<TypePathSegment>>")]
    pub segments: Vec<TypePathSegment>,
}

impl TypePath {
    /// Returns the compact path shape used by DefMap resolution.
    ///
    /// Type anchors such as `<T>::Assoc` and `<T as Trait>::Assoc` are real path syntax, but
    /// there is no honest way to represent the anchor as plain DefMap segments. Callers that need
    /// associated type semantics should handle the anchored shape directly instead of resolving an
    /// empty fallback path.
    pub fn as_def_map_path(&self) -> Option<Path> {
        if self.anchor.is_some() {
            return None;
        }

        Some(Path {
            absolute: self.absolute,
            segments: self
                .segments
                .iter()
                .map(|segment| PathSegment::from_syntax_name(&segment.name))
                .collect(),
        })
    }

    /// Returns the DefMap path prefix ending at `end_idx`.
    ///
    /// Like [`Self::as_def_map_path`], anchored paths return `None` because their prefix carries
    /// type syntax that DefMap paths cannot preserve.
    pub fn as_def_map_path_prefix(&self, end_idx: usize) -> Option<Path> {
        if self.anchor.is_some() {
            return None;
        }

        Some(Path {
            absolute: self.absolute,
            segments: self
                .segments
                .iter()
                .take(end_idx.saturating_add(1))
                .map(|segment| PathSegment::from_syntax_name(&segment.name))
                .collect(),
        })
    }

    /// Returns the name of a single-segment relative path.
    pub fn single_name(&self) -> Option<&Name> {
        if self.anchor.is_some() || self.absolute || self.segments.len() != 1 {
            return None;
        }

        self.segments.first().map(|segment| &segment.name)
    }

    pub fn is_self_type(&self) -> bool {
        self.single_name()
            .is_some_and(|name| name.as_str() == "Self")
    }

    /// Returns true when any path segment or anchor contains explicit generic arguments.
    pub fn has_generic_args(&self) -> bool {
        self.anchor
            .as_ref()
            .is_some_and(TypePathAnchor::has_generic_args)
            || self.segments.iter().any(|segment| !segment.args.is_empty())
    }

    /// Returns true when this path or anchor mentions one of the provided type parameter names.
    pub fn mentions_type_param(&self, params: &[&str]) -> bool {
        self.anchor
            .as_ref()
            .is_some_and(|anchor| anchor.mentions_type_param(params))
            || self.segments.iter().any(|segment| {
                params.contains(&segment.name.as_str())
                    || segment
                        .args
                        .iter()
                        .any(|arg| arg.mentions_type_param(params))
            })
    }
}

impl fmt::Display for TypePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        TypeFormatter::canonical().fmt_type_path(self, f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum TypePathAnchor {
    /// `<T>::Assoc`.
    Type(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<TypeRef>>")] Box<TypeRef>),
    /// `<T as Trait>::Assoc`.
    QualifiedTrait {
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<TypeRef>>")]
        self_ty: Box<TypeRef>,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<TypeRef>>")]
        trait_ty: Box<TypeRef>,
    },
}

impl TypePathAnchor {
    pub fn from_parts(self_ty: TypeRef, trait_ty: Option<TypeRef>) -> Self {
        match trait_ty {
            Some(trait_ty) => Self::QualifiedTrait {
                self_ty: Box::new(self_ty),
                trait_ty: Box::new(trait_ty),
            },
            None => Self::Type(Box::new(self_ty)),
        }
    }

    pub fn has_generic_args(&self) -> bool {
        match self {
            Self::Type(ty) => ty.has_generic_args(),
            Self::QualifiedTrait { self_ty, trait_ty } => {
                self_ty.has_generic_args() || trait_ty.has_generic_args()
            }
        }
    }

    pub fn mentions_type_param(&self, params: &[&str]) -> bool {
        match self {
            Self::Type(ty) => ty.mentions_type_param(params),
            Self::QualifiedTrait { self_ty, trait_ty } => {
                self_ty.mentions_type_param(params) || trait_ty.mentions_type_param(params)
            }
        }
    }
}

impl fmt::Display for TypePathAnchor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        TypeFormatter::canonical().fmt_type_path_anchor(self, f)
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
        TypeFormatter::canonical().fmt_type_path_segment(self, f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum GenericArg {
    Type(#[wincode(with = "rg_wincode_utils::WincodeDynamic<TypeRef>")] TypeRef),
    Lifetime(Name),
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
        TypeFormatter::canonical().fmt_generic_arg(self, f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum TypeBound {
    Trait(#[wincode(with = "rg_wincode_utils::WincodeDynamic<TypeRef>")] TypeRef),
    Lifetime(Name),
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

    /// Displays one `+`-separated bound list through a semantic-name policy.
    pub fn display_list_with<F>(bounds: &[Self], names: F) -> TypeBoundListDisplay<'_, F>
    where
        F: TypeNameFormatter,
    {
        TypeBoundListDisplay { bounds, names }
    }
}

impl fmt::Display for TypeBound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        TypeFormatter::canonical().fmt_type_bound(self, f)
    }
}

/// Controls how canonical semantic names are written inside type syntax.
///
/// `TypeRef` owns the structural traversal. Renderers that know about a source edition can supply
/// only this policy instead of reimplementing tuples, paths, generic arguments, and bounds.
pub trait TypeNameFormatter {
    fn fmt_name(&self, name: &Name, f: &mut fmt::Formatter<'_>) -> fmt::Result;
}

/// Borrowed display of one type using a caller-provided name policy.
pub struct TypeRefDisplay<'a, F> {
    ty: &'a TypeRef,
    names: F,
}

impl<F> fmt::Display for TypeRefDisplay<'_, F>
where
    F: TypeNameFormatter,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        TypeFormatter::new(&self.names).fmt_type_ref(self.ty, f)
    }
}

/// Borrowed display of one `+`-separated bound list using a caller-provided name policy.
pub struct TypeBoundListDisplay<'a, F> {
    bounds: &'a [TypeBound],
    names: F,
}

impl<F> fmt::Display for TypeBoundListDisplay<'_, F>
where
    F: TypeNameFormatter,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        TypeFormatter::new(&self.names).fmt_type_bounds(self.bounds, f)
    }
}

struct CanonicalTypeNames;

impl TypeNameFormatter for CanonicalTypeNames {
    fn fmt_name(&self, name: &Name, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(name, f)
    }
}

/// One structural traversal serves both canonical/debug output and edition-aware presentation.
struct TypeFormatter<'a, F> {
    names: &'a F,
}

impl<'a, F> TypeFormatter<'a, F>
where
    F: TypeNameFormatter,
{
    fn new(names: &'a F) -> Self {
        Self { names }
    }

    fn fmt_type_ref(&self, ty: &TypeRef, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match ty {
            TypeRef::Unknown(text) if text.is_empty() => f.write_str("<unknown>"),
            TypeRef::Unknown(text) => write!(f, "<unsupported:{text}>"),
            TypeRef::Never => f.write_str("!"),
            TypeRef::Unit => f.write_str("()"),
            TypeRef::Infer => f.write_str("_"),
            TypeRef::Path(path) => self.fmt_type_path(path, f),
            TypeRef::Tuple(types) => {
                f.write_str("(")?;
                for (index, ty) in types.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    self.fmt_type_ref(ty, f)?;
                }
                if types.len() == 1 {
                    f.write_str(",")?;
                }
                f.write_str(")")
            }
            TypeRef::Reference {
                lifetime,
                mutability,
                inner,
            } => {
                f.write_str("&")?;
                if let Some(lifetime) = lifetime {
                    self.names.fmt_name(lifetime, f)?;
                    f.write_str(" ")?;
                }
                if matches!(mutability, Mutability::Mutable) {
                    f.write_str("mut ")?;
                }
                self.fmt_type_ref(inner, f)
            }
            TypeRef::RawPointer { mutability, inner } => {
                f.write_str(match mutability {
                    Mutability::Shared => "*const ",
                    Mutability::Mutable => "*mut ",
                })?;
                self.fmt_type_ref(inner, f)
            }
            TypeRef::Slice(inner) => {
                f.write_str("[")?;
                self.fmt_type_ref(inner, f)?;
                f.write_str("]")
            }
            TypeRef::Array { inner, len } => {
                f.write_str("[")?;
                self.fmt_type_ref(inner, f)?;
                write!(f, "; {}]", len.as_deref().unwrap_or("<unknown>"))
            }
            TypeRef::FnPointer { params, ret } => {
                f.write_str("fn(")?;
                for (index, param) in params.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    self.fmt_type_ref(param, f)?;
                }
                f.write_str(")")?;
                if !matches!(ret.as_ref(), TypeRef::Unit) {
                    f.write_str(" -> ")?;
                    self.fmt_type_ref(ret, f)?;
                }
                Ok(())
            }
            TypeRef::ImplTrait(bounds) => {
                f.write_str("impl ")?;
                self.fmt_type_bounds(bounds, f)
            }
            TypeRef::DynTrait(bounds) => {
                f.write_str("dyn ")?;
                self.fmt_type_bounds(bounds, f)
            }
        }
    }

    fn fmt_type_path(&self, path: &TypePath, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(anchor) = &path.anchor {
            self.fmt_type_path_anchor(anchor, f)?;
            if !path.segments.is_empty() {
                f.write_str("::")?;
            }
        } else if path.absolute {
            f.write_str("::")?;
        }

        for (index, segment) in path.segments.iter().enumerate() {
            if index > 0 {
                f.write_str("::")?;
            }
            self.fmt_type_path_segment(segment, f)?;
        }
        Ok(())
    }

    fn fmt_type_path_anchor(
        &self,
        anchor: &TypePathAnchor,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        f.write_str("<")?;
        match anchor {
            TypePathAnchor::Type(ty) => self.fmt_type_ref(ty, f)?,
            TypePathAnchor::QualifiedTrait { self_ty, trait_ty } => {
                self.fmt_type_ref(self_ty, f)?;
                f.write_str(" as ")?;
                self.fmt_type_ref(trait_ty, f)?;
            }
        }
        f.write_str(">")
    }

    fn fmt_type_path_segment(
        &self,
        segment: &TypePathSegment,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        self.names.fmt_name(&segment.name, f)?;
        if let [GenericArg::FnTraitArgs { params, ret }] = segment.args.as_slice() {
            return self.fmt_fn_trait_args(params, ret, f);
        }

        if !segment.args.is_empty() {
            f.write_str("<")?;
            for (index, arg) in segment.args.iter().enumerate() {
                if index > 0 {
                    f.write_str(", ")?;
                }
                self.fmt_generic_arg(arg, f)?;
            }
            f.write_str(">")?;
        }
        Ok(())
    }

    fn fmt_generic_arg(&self, arg: &GenericArg, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match arg {
            GenericArg::Type(ty) => self.fmt_type_ref(ty, f),
            GenericArg::Lifetime(lifetime) => self.names.fmt_name(lifetime, f),
            GenericArg::Const(value) => f.write_str(value),
            GenericArg::FnTraitArgs { params, ret } => self.fmt_fn_trait_args(params, ret, f),
            GenericArg::AssocType { name, ty } => {
                self.names.fmt_name(name, f)?;
                if let Some(ty) = ty {
                    f.write_str(" = ")?;
                    self.fmt_type_ref(ty, f)?;
                }
                Ok(())
            }
            GenericArg::Unsupported(text) => write!(f, "<unsupported:{text}>"),
        }
    }

    fn fmt_fn_trait_args(
        &self,
        params: &[TypeRef],
        ret: &TypeRef,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        f.write_str("(")?;
        for (index, param) in params.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            self.fmt_type_ref(param, f)?;
        }
        f.write_str(")")?;
        if !matches!(ret, TypeRef::Unit) {
            f.write_str(" -> ")?;
            self.fmt_type_ref(ret, f)?;
        }
        Ok(())
    }

    fn fmt_type_bound(&self, bound: &TypeBound, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match bound {
            TypeBound::Trait(ty) => self.fmt_type_ref(ty, f),
            TypeBound::Lifetime(lifetime) => self.names.fmt_name(lifetime, f),
            TypeBound::Unsupported(text) => write!(f, "<unsupported:{text}>"),
        }
    }

    fn fmt_type_bounds(&self, bounds: &[TypeBound], f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, bound) in bounds.iter().enumerate() {
            if index > 0 {
                f.write_str(" + ")?;
            }
            self.fmt_type_bound(bound, f)?;
        }
        Ok(())
    }
}

impl TypeFormatter<'static, CanonicalTypeNames> {
    fn canonical() -> Self {
        static NAMES: CanonicalTypeNames = CanonicalTypeNames;
        Self::new(&NAMES)
    }
}

#[cfg(test)]
mod tests {
    use rg_parse::{Span, TextSpan};
    use rg_text::Name;

    use super::{TypePath, TypePathAnchor, TypePathSegment, TypeRef};

    #[test]
    fn builds_def_map_paths_from_type_paths() {
        let cases = [
            (
                "relative keywords and names",
                type_path(false, &["crate", "super", "self", "User", "Self"]),
                "crate::super::self::User::Self",
            ),
            (
                "absolute path",
                type_path(true, &["api", "User"]),
                "::api::User",
            ),
        ];

        for (label, path, expected) in cases {
            let actual = path.as_def_map_path().map(|path| path.to_string());
            assert_eq!(actual.as_deref(), Some(expected), "{label}");
        }
    }

    #[test]
    fn projects_path_type_refs_into_def_map_paths() {
        let cases = [
            (
                "path type",
                TypeRef::Path(type_path(false, &["User"])),
                Some("User"),
            ),
            ("non-path type", TypeRef::Infer, None),
        ];

        for (label, ty, expected) in cases {
            let actual = ty.as_def_map_path().map(|path| path.to_string());
            assert_eq!(actual.as_deref(), expected, "{label}");
        }
    }

    #[test]
    fn builds_def_map_path_prefixes() {
        let path = type_path(false, &["api", "User", "Id"]);

        assert_eq!(
            path.as_def_map_path_prefix(1)
                .map(|path| path.to_string())
                .as_deref(),
            Some("api::User")
        );
    }

    #[test]
    fn anchored_type_paths_have_no_def_map_path_projection() {
        let path = TypePath {
            source_span: span(),
            absolute: false,
            anchor: Some(TypePathAnchor::Type(Box::new(TypeRef::Path(type_path(
                false,
                &["T"],
            ))))),
            segments: vec![TypePathSegment {
                name: Name::new("Assoc"),
                args: Vec::new(),
                span: span(),
            }],
        };

        assert_eq!(path.as_def_map_path(), None);
        assert_eq!(path.as_def_map_path_prefix(0), None);
    }

    fn type_path(absolute: bool, names: &[&str]) -> TypePath {
        TypePath {
            source_span: span(),
            absolute,
            anchor: None,
            segments: names
                .iter()
                .map(|name| TypePathSegment {
                    name: Name::new(*name),
                    args: Vec::new(),
                    span: span(),
                })
                .collect(),
        }
    }

    fn span() -> Span {
        Span {
            text: TextSpan { start: 0, end: 0 },
        }
    }
}
