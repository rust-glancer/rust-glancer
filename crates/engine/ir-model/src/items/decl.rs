//! Syntax-level declaration facts stored in item trees.
//!
//! These types preserve what the user wrote in signatures and item headers. Name resolution,
//! type solving, and semantic ownership are left to later IR layers.

use rg_std::{MemorySize, Shrink};
use std::fmt;
use wincode::{SchemaRead, SchemaWrite};

use rg_parse::Span;
use rg_text::Name;

use super::{Documentation, ItemTreeId, TypeBound, TypeRef, VisibilityLevel};
use crate::Mutability;

/// Generic parameters as they were written on one item declaration.
///
/// Lifetimes form a separate leading group. Type and const parameters share one list because they
/// can interleave: `struct Buffer<T, const N: usize, U>` must retain the order `T, N, U`. Splitting
/// them into separate type and const lists would lose the positional order later used by
/// `Buffer<Key, 16, Value>`.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct GenericParams {
    pub lifetimes: Vec<LifetimeParamData>,
    pub type_or_consts: Vec<TypeOrConstParamData>,
    pub where_predicates: Vec<WherePredicate>,
}

impl GenericParams {
    /// Iterates type parameter names in declaration order.
    pub fn type_param_names(&self) -> impl Iterator<Item = &Name> {
        self.types().map(|param| &param.name)
    }

    pub fn types(&self) -> impl DoubleEndedIterator<Item = &TypeParamData> {
        self.type_or_consts.iter().filter_map(|param| match param {
            TypeOrConstParamData::Type(param) => Some(param),
            TypeOrConstParamData::Const(_) => None,
        })
    }

    pub fn consts(&self) -> impl DoubleEndedIterator<Item = &ConstParamData> {
        self.type_or_consts.iter().filter_map(|param| match param {
            TypeOrConstParamData::Type(_) => None,
            TypeOrConstParamData::Const(param) => Some(param),
        })
    }

    pub fn push_type(&mut self, param: TypeParamData) {
        self.type_or_consts.push(TypeOrConstParamData::Type(param));
    }

    pub fn push_const(&mut self, param: ConstParamData) {
        self.type_or_consts.push(TypeOrConstParamData::Const(param));
    }

    pub fn is_empty(&self) -> bool {
        self.lifetimes.is_empty()
            && self.type_or_consts.is_empty()
            && self.where_predicates.is_empty()
    }
}

impl fmt::Display for GenericParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut params = Vec::new();

        params.extend(self.lifetimes.iter().map(|param| {
            if param.bounds.is_empty() {
                param.name.to_string()
            } else {
                format!(
                    "{}: {}",
                    param.name,
                    param
                        .bounds
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" + ")
                )
            }
        }));
        params.extend(self.type_or_consts.iter().map(|param| match param {
            TypeOrConstParamData::Type(param) => {
                let mut text = param.name.to_string();
                if !param.bounds.is_empty() {
                    text.push_str(": ");
                    text.push_str(
                        &param
                            .bounds
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(" + "),
                    );
                }
                if let Some(default) = &param.default {
                    text.push_str(" = ");
                    text.push_str(&default.to_string());
                }
                text
            }
            TypeOrConstParamData::Const(param) => {
                let mut text = format!("const {}", param.name);
                if let Some(ty) = &param.ty {
                    text.push_str(": ");
                    text.push_str(&ty.to_string());
                }
                if let Some(default) = &param.default {
                    text.push_str(" = ");
                    text.push_str(default);
                }
                text
            }
        }));

        if !params.is_empty() {
            write!(f, "<{}>", params.join(", "))?;
        }

        if !self.where_predicates.is_empty() {
            write!(f, " where ")?;
            for (idx, predicate) in self.where_predicates.iter().enumerate() {
                if idx > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{predicate}")?;
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct LifetimeParamData {
    pub name: Name,
    pub bounds: Vec<Name>,
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct TypeParamData {
    pub name: Name,
    pub bounds: Vec<TypeBound>,
    pub default: Option<TypeRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ConstParamData {
    pub name: Name,
    pub ty: Option<TypeRef>,
    pub default: Option<String>,
}

/// Type and const parameters in their shared source declaration order.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum TypeOrConstParamData {
    Type(TypeParamData),
    Const(ConstParamData),
}

/// Where-clause predicate that can affect later signature resolution.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum WherePredicate {
    Type { ty: TypeRef, bounds: Vec<TypeBound> },
    Lifetime { lifetime: Name, bounds: Vec<Name> },
    Unsupported(String),
}

impl fmt::Display for WherePredicate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Type { ty, bounds } => write_bound_list(f, &ty.to_string(), bounds),
            Self::Lifetime { lifetime, bounds } => {
                write!(f, "{lifetime}: ")?;
                for (index, bound) in bounds.iter().enumerate() {
                    if index > 0 {
                        write!(f, " + ")?;
                    }
                    write!(f, "{bound}")?;
                }
                Ok(())
            }
            Self::Unsupported(text) => write!(f, "<unsupported:{text}>"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct FunctionItem {
    pub generics: GenericParams,
    pub params: Vec<ParamItem>,
    pub ret_ty: Option<TypeRef>,
    pub qualifiers: FunctionQualifiers,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink,
)]
#[shrink(leaf)]
pub struct FunctionQualifiers {
    pub is_async: bool,
    pub is_const: bool,
    pub is_unsafe: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ParamItem {
    pub pat: String,
    pub ty: Option<TypeRef>,
    pub kind: ParamKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum ParamKind {
    SelfParam(SelfParamKind),
    Normal,
}

/// Receiver form written by a function's self parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum SelfParamKind {
    Value,
    Reference { mutability: crate::Mutability },
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct StructItem {
    pub generics: GenericParams,
    pub fields: FieldList,
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct UnionItem {
    pub generics: GenericParams,
    pub fields: Vec<FieldItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct EnumItem {
    pub generics: GenericParams,
    pub variants: Vec<EnumVariantItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct EnumVariantItem {
    pub name: Name,
    pub span: Span,
    pub name_span: Span,
    pub docs: Option<Documentation>,
    pub fields: FieldList,
}

/// Field shape shared by structs and enum variants.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum FieldList {
    Named(Vec<FieldItem>),
    Tuple(Vec<FieldItem>),
    Unit,
}

impl FieldList {
    pub fn fields(&self) -> &[FieldItem] {
        match self {
            Self::Named(fields) | Self::Tuple(fields) => fields,
            Self::Unit => &[],
        }
    }

    /// Whether this shape introduces a constructor in the value namespace.
    ///
    /// Record-shaped structs and variants are named only through the type namespace. Tuple and
    /// unit shapes additionally provide a callable or bare value constructor.
    pub fn has_value_constructor(&self) -> bool {
        matches!(self, Self::Tuple(_) | Self::Unit)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct FieldItem {
    pub key: Option<FieldKey>,
    pub visibility: VisibilityLevel,
    pub ty: TypeRef,
    pub span: Span,
    pub docs: Option<Documentation>,
}

/// User-visible field identity before semantic ownership is known.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum FieldKey {
    Named(Name),
    Tuple(usize),
}

impl FieldKey {
    pub fn declaration_label(&self) -> String {
        match self {
            Self::Named(name) => name.to_string(),
            Self::Tuple(index) => format!("#{index}"),
        }
    }
}

impl fmt::Display for FieldKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Named(name) => write!(f, "{name}"),
            Self::Tuple(index) => write!(f, "{index}"),
        }
    }
}

impl FieldItem {
    /// Returns the user-visible declaration label for this field's key, if one was parsed.
    pub fn key_declaration_label(&self) -> Option<String> {
        self.key.as_ref().map(FieldKey::declaration_label)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct TraitItem {
    pub generics: GenericParams,
    pub super_traits: Vec<TypeBound>,
    pub items: Vec<ItemTreeId>,
    pub is_unsafe: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ImplItem {
    pub generics: GenericParams,
    pub trait_ref: Option<TypeRef>,
    pub self_ty: TypeRef,
    pub items: Vec<ItemTreeId>,
    pub is_unsafe: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct TypeAliasItem {
    pub generics: GenericParams,
    pub bounds: Vec<TypeBound>,
    pub aliased_ty: Option<TypeRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ConstItem {
    pub generics: GenericParams,
    pub ty: Option<TypeRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct StaticItem {
    pub ty: Option<TypeRef>,
    pub mutability: Mutability,
}

fn write_bound_list(
    f: &mut fmt::Formatter<'_>,
    subject: &str,
    bounds: &[TypeBound],
) -> fmt::Result {
    write!(f, "{subject}")?;
    if !bounds.is_empty() {
        write!(f, ": ")?;
        for (idx, bound) in bounds.iter().enumerate() {
            if idx > 0 {
                write!(f, " + ")?;
            }
            write!(f, "{bound}")?;
        }
    }
    Ok(())
}
