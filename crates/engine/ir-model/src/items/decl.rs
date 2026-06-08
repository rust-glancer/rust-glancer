//! Syntax-level declaration facts stored in item trees.
//!
//! These types preserve what the user wrote in signatures and item headers. Name resolution,
//! type solving, and semantic ownership are left to later IR layers.

use rg_std::MemorySize;
use std::fmt;
use wincode::{SchemaRead, SchemaWrite};

use rg_parse::Span;
use rg_text::Name;

use super::{Documentation, ItemTreeId, Mutability, TypeBound, TypeRef, VisibilityLevel};

/// Generic parameter data attached to an item declaration.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize)]
pub struct GenericParams {
    pub lifetimes: Vec<LifetimeParamData>,
    pub types: Vec<TypeParamData>,
    pub consts: Vec<ConstParamData>,
    pub where_predicates: Vec<WherePredicate>,
}

impl GenericParams {
    pub fn shrink_to_fit(&mut self) {
        self.lifetimes.shrink_to_fit();
        for param in &mut self.lifetimes {
            param.shrink_to_fit();
        }
        self.types.shrink_to_fit();
        for param in &mut self.types {
            param.shrink_to_fit();
        }
        self.consts.shrink_to_fit();
        for param in &mut self.consts {
            param.shrink_to_fit();
        }
        self.where_predicates.shrink_to_fit();
        for predicate in &mut self.where_predicates {
            predicate.shrink_to_fit();
        }
    }
}

impl fmt::Display for GenericParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut params = Vec::new();

        params.extend(self.lifetimes.iter().map(|param| {
            if param.bounds.is_empty() {
                param.name.to_string()
            } else {
                format!("{}: {}", param.name, param.bounds.join(" + "))
            }
        }));
        params.extend(self.types.iter().map(|param| {
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
        }));
        params.extend(self.consts.iter().map(|param| {
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

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct LifetimeParamData {
    pub name: Name,
    pub bounds: Vec<String>,
}

impl LifetimeParamData {
    fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        self.bounds.shrink_to_fit();
        for bound in &mut self.bounds {
            bound.shrink_to_fit();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct TypeParamData {
    pub name: Name,
    pub bounds: Vec<TypeBound>,
    pub default: Option<TypeRef>,
}

impl TypeParamData {
    fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        self.bounds.shrink_to_fit();
        for bound in &mut self.bounds {
            bound.shrink_to_fit();
        }
        if let Some(default) = &mut self.default {
            default.shrink_to_fit();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct ConstParamData {
    pub name: Name,
    pub ty: Option<TypeRef>,
    pub default: Option<String>,
}

impl ConstParamData {
    fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        if let Some(ty) = &mut self.ty {
            ty.shrink_to_fit();
        }
        if let Some(default) = &mut self.default {
            default.shrink_to_fit();
        }
    }
}

/// Where-clause predicate that can affect later signature resolution.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub enum WherePredicate {
    Type {
        ty: TypeRef,
        bounds: Vec<TypeBound>,
    },
    Lifetime {
        lifetime: String,
        bounds: Vec<String>,
    },
    Unsupported(String),
}

impl WherePredicate {
    fn shrink_to_fit(&mut self) {
        match self {
            Self::Type { ty, bounds } => {
                ty.shrink_to_fit();
                bounds.shrink_to_fit();
                for bound in bounds {
                    bound.shrink_to_fit();
                }
            }
            Self::Lifetime { lifetime, bounds } => {
                lifetime.shrink_to_fit();
                bounds.shrink_to_fit();
                for bound in bounds {
                    bound.shrink_to_fit();
                }
            }
            Self::Unsupported(text) => text.shrink_to_fit(),
        }
    }
}

impl fmt::Display for WherePredicate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Type { ty, bounds } => write_bound_list(f, &ty.to_string(), bounds),
            Self::Lifetime { lifetime, bounds } => {
                write!(f, "{lifetime}: {}", bounds.join(" + "))
            }
            Self::Unsupported(text) => write!(f, "<unsupported:{text}>"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct FunctionItem {
    pub generics: GenericParams,
    pub params: Vec<ParamItem>,
    pub ret_ty: Option<TypeRef>,
    pub qualifiers: FunctionQualifiers,
}

impl FunctionItem {
    pub fn shrink_to_fit(&mut self) {
        self.generics.shrink_to_fit();
        self.params.shrink_to_fit();
        for param in &mut self.params {
            param.shrink_to_fit();
        }
        if let Some(ret_ty) = &mut self.ret_ty {
            ret_ty.shrink_to_fit();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize)]
pub struct FunctionQualifiers {
    pub is_async: bool,
    pub is_const: bool,
    pub is_unsafe: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct ParamItem {
    pub pat: String,
    pub ty: Option<TypeRef>,
    pub kind: ParamKind,
}

impl ParamItem {
    pub fn shrink_to_fit(&mut self) {
        self.pat.shrink_to_fit();
        if let Some(ty) = &mut self.ty {
            ty.shrink_to_fit();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
#[memsize(leaf)]
pub enum ParamKind {
    SelfParam,
    Normal,
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct StructItem {
    pub generics: GenericParams,
    pub fields: FieldList,
}

impl StructItem {
    pub fn shrink_to_fit(&mut self) {
        self.generics.shrink_to_fit();
        self.fields.shrink_to_fit();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct UnionItem {
    pub generics: GenericParams,
    pub fields: Vec<FieldItem>,
}

impl UnionItem {
    pub fn shrink_to_fit(&mut self) {
        self.generics.shrink_to_fit();
        self.fields.shrink_to_fit();
        for field in &mut self.fields {
            field.shrink_to_fit();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct EnumItem {
    pub generics: GenericParams,
    pub variants: Vec<EnumVariantItem>,
}

impl EnumItem {
    pub fn shrink_to_fit(&mut self) {
        self.generics.shrink_to_fit();
        self.variants.shrink_to_fit();
        for variant in &mut self.variants {
            variant.shrink_to_fit();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct EnumVariantItem {
    pub name: Name,
    pub span: Span,
    pub name_span: Span,
    pub docs: Option<Documentation>,
    pub fields: FieldList,
}

impl EnumVariantItem {
    pub fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        if let Some(docs) = &mut self.docs {
            docs.shrink_to_fit();
        }
        self.fields.shrink_to_fit();
    }
}

/// Field shape shared by structs and enum variants.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
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

    pub fn shrink_to_fit(&mut self) {
        match self {
            Self::Named(fields) | Self::Tuple(fields) => {
                fields.shrink_to_fit();
                for field in fields {
                    field.shrink_to_fit();
                }
            }
            Self::Unit => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct FieldItem {
    pub key: Option<FieldKey>,
    pub visibility: VisibilityLevel,
    pub ty: TypeRef,
    pub span: Span,
    pub docs: Option<Documentation>,
}

/// User-visible field identity before semantic ownership is known.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
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

    pub fn shrink_to_fit(&mut self) {
        if let Self::Named(name) = self {
            name.shrink_to_fit();
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

    pub fn shrink_to_fit(&mut self) {
        if let Some(key) = &mut self.key {
            key.shrink_to_fit();
        }
        self.ty.shrink_to_fit();
        if let Some(docs) = &mut self.docs {
            docs.shrink_to_fit();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct TraitItem {
    pub generics: GenericParams,
    pub super_traits: Vec<TypeBound>,
    pub items: Vec<ItemTreeId>,
    pub is_unsafe: bool,
}

impl TraitItem {
    pub fn shrink_to_fit(&mut self) {
        self.generics.shrink_to_fit();
        self.super_traits.shrink_to_fit();
        for bound in &mut self.super_traits {
            bound.shrink_to_fit();
        }
        self.items.shrink_to_fit();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct ImplItem {
    pub generics: GenericParams,
    pub trait_ref: Option<TypeRef>,
    pub self_ty: TypeRef,
    pub items: Vec<ItemTreeId>,
    pub is_unsafe: bool,
}

impl ImplItem {
    pub fn shrink_to_fit(&mut self) {
        self.generics.shrink_to_fit();
        if let Some(trait_ref) = &mut self.trait_ref {
            trait_ref.shrink_to_fit();
        }
        self.self_ty.shrink_to_fit();
        self.items.shrink_to_fit();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct TypeAliasItem {
    pub generics: GenericParams,
    pub bounds: Vec<TypeBound>,
    pub aliased_ty: Option<TypeRef>,
}

impl TypeAliasItem {
    pub fn shrink_to_fit(&mut self) {
        self.generics.shrink_to_fit();
        self.bounds.shrink_to_fit();
        for bound in &mut self.bounds {
            bound.shrink_to_fit();
        }
        if let Some(aliased_ty) = &mut self.aliased_ty {
            aliased_ty.shrink_to_fit();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct ConstItem {
    pub generics: GenericParams,
    pub ty: Option<TypeRef>,
}

impl ConstItem {
    pub fn shrink_to_fit(&mut self) {
        self.generics.shrink_to_fit();
        if let Some(ty) = &mut self.ty {
            ty.shrink_to_fit();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct StaticItem {
    pub ty: Option<TypeRef>,
    pub mutability: Mutability,
}

impl StaticItem {
    pub fn shrink_to_fit(&mut self) {
        if let Some(ty) = &mut self.ty {
            ty.shrink_to_fit();
        }
    }
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
