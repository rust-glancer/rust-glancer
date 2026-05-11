use std::fmt;

use ra_syntax::{
    AstNode as _,
    ast::{self, HasGenericArgs},
};

use rg_parse::{LineIndex, Span};
use rg_text::{Name, NameInterner};

use super::normalized_syntax;

/// Syntax-level mutability marker used by lowered declarations and type refs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum Mutability {
    Shared,
    Mutable,
}

impl Mutability {
    pub fn from_mut_token(is_mut: bool) -> Self {
        if is_mut { Self::Mutable } else { Self::Shared }
    }
}

impl fmt::Display for Mutability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shared => write!(f, "shared"),
            Self::Mutable => write!(f, "mut"),
        }
    }
}

/// Unresolved type syntax lowered into the item tree.
///
/// This intentionally stops before semantic resolution. `TypeRef` represents what the user wrote
/// in an item declaration; resolving paths to definitions belongs to later IR layers.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(
    bytecheck(
        bounds(
            __C: rkyv::validation::ArchiveContext + rkyv::validation::SharedContext,
            <__C as rkyv::rancor::Fallible>::Error: rkyv::rancor::Source,
        )
    ),
    serialize_bounds(
        __S: rkyv::ser::Allocator + rkyv::ser::Sharing + rkyv::ser::Writer,
        <__S as rkyv::rancor::Fallible>::Error: rkyv::rancor::Source,
    ),
    deserialize_bounds(
        __D: rkyv::de::Pooling,
        <__D as rkyv::rancor::Fallible>::Error: rkyv::rancor::Source,
    )
)]
pub enum TypeRef {
    Unknown(String),
    Never,
    Unit,
    Infer,
    Path(#[rkyv(omit_bounds)] TypePath),
    Tuple(#[rkyv(omit_bounds)] Vec<TypeRef>),
    Reference {
        lifetime: Option<String>,
        mutability: Mutability,
        #[rkyv(omit_bounds)]
        inner: Box<TypeRef>,
    },
    RawPointer {
        mutability: Mutability,
        #[rkyv(omit_bounds)]
        inner: Box<TypeRef>,
    },
    Slice(#[rkyv(omit_bounds)] Box<TypeRef>),
    Array {
        #[rkyv(omit_bounds)]
        inner: Box<TypeRef>,
        len: Option<String>,
    },
    FnPointer {
        #[rkyv(omit_bounds)]
        params: Vec<TypeRef>,
        #[rkyv(omit_bounds)]
        ret: Box<TypeRef>,
    },
    ImplTrait(#[rkyv(omit_bounds)] Vec<TypeBound>),
    DynTrait(#[rkyv(omit_bounds)] Vec<TypeBound>),
}

impl TypeRef {
    pub fn unknown_from_text(text: impl Into<String>) -> Self {
        Self::Unknown(text.into())
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

    pub fn from_ast(ty: ast::Type, line_index: &LineIndex, interner: &mut NameInterner) -> Self {
        match ty {
            ast::Type::ArrayType(ty) => Self::Array {
                inner: Box::new(
                    ty.ty()
                        .map(|ty| Self::from_ast(ty, line_index, interner))
                        .unwrap_or_else(|| Self::unknown_from_text(normalized_syntax(&ty))),
                ),
                len: ty.const_arg().map(|arg| normalized_syntax(&arg)),
            },
            ast::Type::DynTraitType(ty) => Self::DynTrait(TypeBound::list_from_ast(
                ty.type_bound_list(),
                line_index,
                interner,
            )),
            ast::Type::FnPtrType(ty) => Self::FnPointer {
                params: ty
                    .param_list()
                    .into_iter()
                    .flat_map(|param_list| param_list.params())
                    .map(|param| {
                        param
                            .ty()
                            .map(|ty| Self::from_ast(ty, line_index, interner))
                            .unwrap_or_else(|| Self::Unknown(String::new()))
                    })
                    .collect(),
                ret: Box::new(
                    ty.ret_type()
                        .and_then(|ret_ty| ret_ty.ty())
                        .map(|ty| Self::from_ast(ty, line_index, interner))
                        .unwrap_or(Self::Unit),
                ),
            },
            ast::Type::ForType(ty) => ty
                .ty()
                .map(|ty| Self::from_ast(ty, line_index, interner))
                .unwrap_or_else(|| Self::unknown_from_text(normalized_syntax(&ty))),
            ast::Type::ImplTraitType(ty) => Self::ImplTrait(TypeBound::list_from_ast(
                ty.type_bound_list(),
                line_index,
                interner,
            )),
            ast::Type::InferType(_) => Self::Infer,
            ast::Type::MacroType(ty) => Self::unknown_from_text(normalized_syntax(&ty)),
            ast::Type::NeverType(_) => Self::Never,
            ast::Type::ParenType(ty) => ty
                .ty()
                .map(|ty| Self::from_ast(ty, line_index, interner))
                .unwrap_or_else(|| Self::unknown_from_text(normalized_syntax(&ty))),
            ast::Type::PathType(ty) => ty
                .path()
                .map(|path| TypePath::from_ast(path, line_index, interner))
                .map(Self::Path)
                .unwrap_or_else(|| Self::unknown_from_text(normalized_syntax(&ty))),
            ast::Type::PtrType(ty) => Self::RawPointer {
                mutability: Mutability::from_mut_token(ty.mut_token().is_some()),
                inner: Box::new(
                    ty.ty()
                        .map(|ty| Self::from_ast(ty, line_index, interner))
                        .unwrap_or_else(|| Self::unknown_from_text(normalized_syntax(&ty))),
                ),
            },
            ast::Type::RefType(ty) => Self::Reference {
                lifetime: ty.lifetime().map(|lifetime| normalized_syntax(&lifetime)),
                mutability: Mutability::from_mut_token(ty.mut_token().is_some()),
                inner: Box::new(
                    ty.ty()
                        .map(|ty| Self::from_ast(ty, line_index, interner))
                        .unwrap_or_else(|| Self::unknown_from_text(normalized_syntax(&ty))),
                ),
            },
            ast::Type::SliceType(ty) => Self::Slice(Box::new(
                ty.ty()
                    .map(|ty| Self::from_ast(ty, line_index, interner))
                    .unwrap_or_else(|| Self::unknown_from_text(normalized_syntax(&ty))),
            )),
            ast::Type::TupleType(ty) => {
                let fields = ty
                    .fields()
                    .map(|ty| Self::from_ast(ty, line_index, interner))
                    .collect::<Vec<_>>();
                if fields.is_empty() {
                    Self::Unit
                } else {
                    Self::Tuple(fields)
                }
            }
        }
    }

    pub fn shrink_to_fit(&mut self) {
        match self {
            Self::Unknown(text) => text.shrink_to_fit(),
            Self::Path(path) => path.shrink_to_fit(),
            Self::Tuple(types) => {
                types.shrink_to_fit();
                for ty in types {
                    ty.shrink_to_fit();
                }
            }
            Self::Reference {
                lifetime, inner, ..
            } => {
                if let Some(lifetime) = lifetime {
                    lifetime.shrink_to_fit();
                }
                inner.shrink_to_fit();
            }
            Self::RawPointer { inner, .. } | Self::Slice(inner) => inner.shrink_to_fit(),
            Self::Array { inner, len } => {
                inner.shrink_to_fit();
                if let Some(len) = len {
                    len.shrink_to_fit();
                }
            }
            Self::FnPointer { params, ret } => {
                params.shrink_to_fit();
                for param in params {
                    param.shrink_to_fit();
                }
                ret.shrink_to_fit();
            }
            Self::ImplTrait(bounds) | Self::DynTrait(bounds) => {
                bounds.shrink_to_fit();
                for bound in bounds {
                    bound.shrink_to_fit();
                }
            }
            Self::Never | Self::Unit | Self::Infer => {}
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

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(
    bytecheck(
        bounds(
            __C: rkyv::validation::ArchiveContext + rkyv::validation::SharedContext,
            <__C as rkyv::rancor::Fallible>::Error: rkyv::rancor::Source,
        )
    ),
    serialize_bounds(
        __S: rkyv::ser::Allocator + rkyv::ser::Sharing + rkyv::ser::Writer,
        <__S as rkyv::rancor::Fallible>::Error: rkyv::rancor::Source,
    ),
    deserialize_bounds(
        __D: rkyv::de::Pooling,
        <__D as rkyv::rancor::Fallible>::Error: rkyv::rancor::Source,
    )
)]
pub struct TypePath {
    pub absolute: bool,
    #[rkyv(omit_bounds)]
    pub segments: Vec<TypePathSegment>,
}

impl TypePath {
    pub fn from_ast(path: ast::Path, line_index: &LineIndex, interner: &mut NameInterner) -> Self {
        let absolute = path
            .first_segment()
            .is_some_and(|segment| segment.coloncolon_token().is_some());
        let mut segments = Vec::new();
        Self::collect_segments(&path, line_index, interner, &mut segments);

        Self { absolute, segments }
    }

    fn collect_segments(
        path: &ast::Path,
        line_index: &LineIndex,
        interner: &mut NameInterner,
        segments: &mut Vec<TypePathSegment>,
    ) {
        if let Some(qualifier) = path.qualifier() {
            Self::collect_segments(&qualifier, line_index, interner, segments);
        }

        if let Some(segment) = path.segment() {
            segments.push(TypePathSegment::from_ast(&segment, line_index, interner));
        }
    }

    pub fn shrink_to_fit(&mut self) {
        self.segments.shrink_to_fit();
        for segment in &mut self.segments {
            segment.shrink_to_fit();
        }
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

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(
    bytecheck(
        bounds(
            __C: rkyv::validation::ArchiveContext + rkyv::validation::SharedContext,
            <__C as rkyv::rancor::Fallible>::Error: rkyv::rancor::Source,
        )
    ),
    serialize_bounds(
        __S: rkyv::ser::Allocator + rkyv::ser::Sharing + rkyv::ser::Writer,
        <__S as rkyv::rancor::Fallible>::Error: rkyv::rancor::Source,
    ),
    deserialize_bounds(
        __D: rkyv::de::Pooling,
        <__D as rkyv::rancor::Fallible>::Error: rkyv::rancor::Source,
    )
)]
pub struct TypePathSegment {
    pub name: Name,
    #[rkyv(omit_bounds)]
    pub args: Vec<GenericArg>,
    pub span: Span,
}

impl TypePathSegment {
    fn from_ast(
        segment: &ast::PathSegment,
        line_index: &LineIndex,
        interner: &mut NameInterner,
    ) -> Self {
        let name = segment
            .name_ref()
            .map(|name| interner.intern(name.syntax().text().to_string()))
            .unwrap_or_else(|| interner.intern(normalized_syntax(segment)));
        let span = segment
            .name_ref()
            .map(|name| name.syntax().text_range())
            .unwrap_or_else(|| segment.syntax().text_range());
        let mut args = Vec::new();

        if let Some(arg_list) = segment.generic_arg_list() {
            args.extend(
                arg_list
                    .generic_args()
                    .map(|arg| GenericArg::from_ast(arg, line_index, interner)),
            );
        }

        if let Some(parenthesized_args) = segment.parenthesized_arg_list() {
            args.push(GenericArg::Unsupported(normalized_syntax(
                &parenthesized_args,
            )));
        }

        Self {
            name,
            args,
            span: Span::from_text_range(span),
        }
    }

    pub fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        self.args.shrink_to_fit();
        for arg in &mut self.args {
            arg.shrink_to_fit();
        }
    }
}

impl fmt::Display for TypePathSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)?;
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

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(
    bytecheck(
        bounds(
            __C: rkyv::validation::ArchiveContext + rkyv::validation::SharedContext,
            <__C as rkyv::rancor::Fallible>::Error: rkyv::rancor::Source,
        )
    ),
    serialize_bounds(
        __S: rkyv::ser::Allocator + rkyv::ser::Sharing + rkyv::ser::Writer,
        <__S as rkyv::rancor::Fallible>::Error: rkyv::rancor::Source,
    ),
    deserialize_bounds(
        __D: rkyv::de::Pooling,
        <__D as rkyv::rancor::Fallible>::Error: rkyv::rancor::Source,
    )
)]
pub enum GenericArg {
    Type(#[rkyv(omit_bounds)] TypeRef),
    Lifetime(String),
    Const(String),
    AssocType {
        name: Name,
        #[rkyv(omit_bounds)]
        ty: Option<TypeRef>,
    },
    Unsupported(String),
}

impl GenericArg {
    fn from_ast(arg: ast::GenericArg, line_index: &LineIndex, interner: &mut NameInterner) -> Self {
        match arg {
            ast::GenericArg::AssocTypeArg(arg) => Self::AssocType {
                name: arg
                    .name_ref()
                    .map(|name| interner.intern(name.syntax().text().to_string()))
                    .unwrap_or_else(|| interner.intern("<missing>")),
                ty: arg
                    .ty()
                    .map(|ty| TypeRef::from_ast(ty, line_index, interner)),
            },
            ast::GenericArg::ConstArg(arg) => Self::Const(normalized_syntax(&arg)),
            ast::GenericArg::LifetimeArg(arg) => arg
                .lifetime()
                .map(|lifetime| Self::Lifetime(normalized_syntax(&lifetime)))
                .unwrap_or_else(|| Self::Unsupported(normalized_syntax(&arg))),
            ast::GenericArg::TypeArg(arg) => arg
                .ty()
                .map(|ty| TypeRef::from_ast(ty, line_index, interner))
                .map(Self::Type)
                .unwrap_or_else(|| Self::Unsupported(normalized_syntax(&arg))),
        }
    }

    /// Returns true when this generic argument mentions one of the provided type parameter names.
    pub fn mentions_type_param(&self, params: &[&str]) -> bool {
        match self {
            Self::Type(ty) => ty.mentions_type_param(params),
            Self::AssocType { ty, .. } => {
                ty.as_ref().is_some_and(|ty| ty.mentions_type_param(params))
            }
            Self::Lifetime(_) | Self::Const(_) | Self::Unsupported(_) => false,
        }
    }

    pub fn shrink_to_fit(&mut self) {
        match self {
            Self::Type(ty) => ty.shrink_to_fit(),
            Self::Lifetime(lifetime) | Self::Const(lifetime) | Self::Unsupported(lifetime) => {
                lifetime.shrink_to_fit();
            }
            Self::AssocType { name, ty } => {
                name.shrink_to_fit();
                if let Some(ty) = ty {
                    ty.shrink_to_fit();
                }
            }
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
            Self::Unsupported(text) => write!(f, "<unsupported:{text}>"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(
    bytecheck(
        bounds(
            __C: rkyv::validation::ArchiveContext + rkyv::validation::SharedContext,
            <__C as rkyv::rancor::Fallible>::Error: rkyv::rancor::Source,
        )
    ),
    serialize_bounds(
        __S: rkyv::ser::Allocator + rkyv::ser::Sharing + rkyv::ser::Writer,
        <__S as rkyv::rancor::Fallible>::Error: rkyv::rancor::Source,
    ),
    deserialize_bounds(
        __D: rkyv::de::Pooling,
        <__D as rkyv::rancor::Fallible>::Error: rkyv::rancor::Source,
    )
)]
pub enum TypeBound {
    Trait(#[rkyv(omit_bounds)] TypeRef),
    Lifetime(String),
    Unsupported(String),
}

impl TypeBound {
    pub fn list_from_ast(
        bound_list: Option<ast::TypeBoundList>,
        line_index: &LineIndex,
        interner: &mut NameInterner,
    ) -> Vec<Self> {
        bound_list
            .into_iter()
            .flat_map(|bound_list| bound_list.bounds())
            .map(|bound| Self::from_ast(bound, line_index, interner))
            .collect()
    }

    fn from_ast(
        bound: ast::TypeBound,
        line_index: &LineIndex,
        interner: &mut NameInterner,
    ) -> Self {
        if let Some(lifetime) = bound.lifetime() {
            return Self::Lifetime(normalized_syntax(&lifetime));
        }

        if let Some(ty) = bound.ty() {
            return Self::Trait(TypeRef::from_ast(ty, line_index, interner));
        }

        Self::Unsupported(normalized_syntax(&bound))
    }

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

    pub fn shrink_to_fit(&mut self) {
        match self {
            Self::Trait(ty) => ty.shrink_to_fit(),
            Self::Lifetime(lifetime) | Self::Unsupported(lifetime) => lifetime.shrink_to_fit(),
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
