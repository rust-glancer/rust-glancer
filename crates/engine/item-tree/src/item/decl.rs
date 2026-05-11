//! Syntax-level declaration facts stored in item trees.
//!
//! These types preserve what the user wrote in signatures and item headers. Name resolution,
//! type solving, and semantic ownership are left to later IR layers.

use std::fmt;

use ra_syntax::{
    AstNode as _,
    ast::{self, HasGenericParams, HasName, HasTypeBounds, HasVisibility},
};

use rg_parse::{LineIndex, Span};
use rg_text::{Name, NameInterner};

use super::{
    Documentation, ItemTreeId, Mutability, TypeBound, TypeRef, VisibilityLevel, normalized_syntax,
};

/// Generic parameter data attached to an item declaration.
#[derive(
    Debug, Clone, PartialEq, Eq, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct GenericParams {
    pub lifetimes: Vec<LifetimeParamData>,
    pub types: Vec<TypeParamData>,
    pub consts: Vec<ConstParamData>,
    pub where_predicates: Vec<WherePredicate>,
}

impl GenericParams {
    pub fn from_ast<T>(item: &T, line_index: &LineIndex, interner: &mut NameInterner) -> Self
    where
        T: HasGenericParams,
    {
        let mut params = Self::default();

        if let Some(param_list) = item.generic_param_list() {
            for param in param_list.generic_params() {
                match param {
                    ast::GenericParam::ConstParam(param) => {
                        params.consts.push(ConstParamData {
                            name: param
                                .name()
                                .map(|name| interner.intern(name.text()))
                                .unwrap_or_else(|| interner.intern("<missing>")),
                            ty: param
                                .ty()
                                .map(|ty| TypeRef::from_ast(ty, line_index, interner)),
                            default: param.default_val().map(|value| normalized_syntax(&value)),
                        });
                    }
                    ast::GenericParam::LifetimeParam(param) => {
                        params.lifetimes.push(LifetimeParamData {
                            name: param
                                .lifetime()
                                .map(|lifetime| interner.intern(normalized_syntax(&lifetime)))
                                .unwrap_or_else(|| interner.intern("<missing>")),
                            bounds: lifetime_bounds_from_ast(param.type_bound_list()),
                        });
                    }
                    ast::GenericParam::TypeParam(param) => {
                        params.types.push(TypeParamData {
                            name: param
                                .name()
                                .map(|name| interner.intern(name.text()))
                                .unwrap_or_else(|| interner.intern("<missing>")),
                            bounds: TypeBound::list_from_ast(
                                param.type_bound_list(),
                                line_index,
                                interner,
                            ),
                            default: param
                                .default_type()
                                .map(|ty| TypeRef::from_ast(ty, line_index, interner)),
                        });
                    }
                }
            }
        }

        if let Some(where_clause) = item.where_clause() {
            params.where_predicates = where_clause
                .predicates()
                .map(|predicate| WherePredicate::from_ast(predicate, line_index, interner))
                .collect();
        }

        params
    }

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

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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
    fn from_ast(
        predicate: ast::WherePred,
        line_index: &LineIndex,
        interner: &mut NameInterner,
    ) -> Self {
        if let Some(lifetime) = predicate.lifetime() {
            return Self::Lifetime {
                lifetime: normalized_syntax(&lifetime),
                bounds: lifetime_bounds_from_ast(predicate.type_bound_list()),
            };
        }

        if let Some(ty) = predicate.ty() {
            return Self::Type {
                ty: TypeRef::from_ast(ty, line_index, interner),
                bounds: TypeBound::list_from_ast(predicate.type_bound_list(), line_index, interner),
            };
        }

        Self::Unsupported(normalized_syntax(&predicate))
    }

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

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct FunctionItem {
    pub generics: GenericParams,
    pub params: Vec<ParamItem>,
    pub ret_ty: Option<TypeRef>,
    pub qualifiers: FunctionQualifiers,
}

impl FunctionItem {
    pub fn from_ast(item: &ast::Fn, line_index: &LineIndex, interner: &mut NameInterner) -> Self {
        Self {
            generics: GenericParams::from_ast(item, line_index, interner),
            params: ParamItem::list_from_ast(item.param_list(), line_index, interner),
            ret_ty: item
                .ret_type()
                .and_then(|ret_ty| ret_ty.ty())
                .map(|ty| TypeRef::from_ast(ty, line_index, interner)),
            qualifiers: FunctionQualifiers {
                is_async: item.async_token().is_some(),
                is_const: item.const_token().is_some(),
                is_unsafe: item.unsafe_token().is_some(),
            },
        }
    }

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

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct FunctionQualifiers {
    pub is_async: bool,
    pub is_const: bool,
    pub is_unsafe: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ParamItem {
    pub pat: String,
    pub ty: Option<TypeRef>,
    pub kind: ParamKind,
}

impl ParamItem {
    pub fn list_from_ast(
        param_list: Option<ast::ParamList>,
        line_index: &LineIndex,
        interner: &mut NameInterner,
    ) -> Vec<Self> {
        let Some(param_list) = param_list else {
            return Vec::new();
        };

        let mut params = Vec::new();

        if let Some(self_param) = param_list.self_param() {
            params.push(Self {
                pat: normalized_syntax(&self_param),
                ty: self_param
                    .ty()
                    .map(|ty| TypeRef::from_ast(ty, line_index, interner)),
                kind: ParamKind::SelfParam,
            });
        }

        for param in param_list.params() {
            params.push(Self {
                pat: param
                    .pat()
                    .map(|pat| normalized_syntax(&pat))
                    .unwrap_or_else(|| "<missing>".to_string()),
                ty: param
                    .ty()
                    .map(|ty| TypeRef::from_ast(ty, line_index, interner)),
                kind: ParamKind::Normal,
            });
        }

        params
    }

    fn shrink_to_fit(&mut self) {
        self.pat.shrink_to_fit();
        if let Some(ty) = &mut self.ty {
            ty.shrink_to_fit();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ParamKind {
    SelfParam,
    Normal,
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct StructItem {
    pub generics: GenericParams,
    pub fields: FieldList,
}

impl StructItem {
    pub fn from_ast(
        item: &ast::Struct,
        line_index: &LineIndex,
        interner: &mut NameInterner,
    ) -> Self {
        Self {
            generics: GenericParams::from_ast(item, line_index, interner),
            fields: FieldList::from_ast(item.field_list(), line_index, interner),
        }
    }

    pub fn shrink_to_fit(&mut self) {
        self.generics.shrink_to_fit();
        self.fields.shrink_to_fit();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct UnionItem {
    pub generics: GenericParams,
    pub fields: Vec<FieldItem>,
}

impl UnionItem {
    pub fn from_ast(
        item: &ast::Union,
        line_index: &LineIndex,
        interner: &mut NameInterner,
    ) -> Self {
        Self {
            generics: GenericParams::from_ast(item, line_index, interner),
            fields: item
                .record_field_list()
                .map(|fields| FieldItem::record_list_from_ast(fields, line_index, interner))
                .unwrap_or_default(),
        }
    }

    pub fn shrink_to_fit(&mut self) {
        self.generics.shrink_to_fit();
        self.fields.shrink_to_fit();
        for field in &mut self.fields {
            field.shrink_to_fit();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct EnumItem {
    pub generics: GenericParams,
    pub variants: Vec<EnumVariantItem>,
}

impl EnumItem {
    pub fn from_ast(item: &ast::Enum, line_index: &LineIndex, interner: &mut NameInterner) -> Self {
        Self {
            generics: GenericParams::from_ast(item, line_index, interner),
            variants: item
                .variant_list()
                .map(|variant_list| {
                    variant_list
                        .variants()
                        .map(|variant| EnumVariantItem::from_ast(variant, line_index, interner))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    pub fn shrink_to_fit(&mut self) {
        self.generics.shrink_to_fit();
        self.variants.shrink_to_fit();
        for variant in &mut self.variants {
            variant.shrink_to_fit();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct EnumVariantItem {
    pub name: Name,
    pub span: Span,
    pub name_span: Span,
    pub docs: Option<Documentation>,
    pub fields: FieldList,
}

impl EnumVariantItem {
    fn from_ast(
        variant: ast::Variant,
        line_index: &LineIndex,
        interner: &mut NameInterner,
    ) -> Self {
        let name = variant.name();
        let span = Span::from_text_range(variant.syntax().text_range());
        let name_span = name
            .as_ref()
            .map(|name| Span::from_text_range(name.syntax().text_range()))
            .unwrap_or(span);

        Self {
            name: name
                .map(|name| interner.intern(name.text()))
                .unwrap_or_else(|| interner.intern("<missing>")),
            span,
            name_span,
            docs: Documentation::from_ast(&variant),
            fields: FieldList::from_ast(variant.field_list(), line_index, interner),
        }
    }

    pub fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        if let Some(docs) = &mut self.docs {
            docs.shrink_to_fit();
        }
        self.fields.shrink_to_fit();
    }
}

/// Field shape shared by structs and enum variants.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum FieldList {
    Named(Vec<FieldItem>),
    Tuple(Vec<FieldItem>),
    Unit,
}

impl FieldList {
    pub fn from_ast(
        field_list: Option<ast::FieldList>,
        line_index: &LineIndex,
        interner: &mut NameInterner,
    ) -> Self {
        match field_list {
            Some(ast::FieldList::RecordFieldList(fields)) => Self::Named(
                FieldItem::record_list_from_ast(fields, line_index, interner),
            ),
            Some(ast::FieldList::TupleFieldList(fields)) => {
                Self::Tuple(FieldItem::tuple_list_from_ast(fields, line_index, interner))
            }
            None => Self::Unit,
        }
    }

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

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct FieldItem {
    pub key: Option<FieldKey>,
    pub visibility: VisibilityLevel,
    pub ty: TypeRef,
    pub span: Span,
    pub docs: Option<Documentation>,
}

/// User-visible field identity before semantic ownership is known.
#[derive(Debug, Clone, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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
    fn record_list_from_ast(
        fields: ast::RecordFieldList,
        line_index: &LineIndex,
        interner: &mut NameInterner,
    ) -> Vec<Self> {
        fields
            .fields()
            .map(|field| {
                let name = field.name();
                let span = name
                    .as_ref()
                    .map(|name| name.syntax().text_range())
                    .unwrap_or_else(|| field.syntax().text_range());

                Self {
                    key: name.map(|name| FieldKey::Named(interner.intern(name.text()))),
                    visibility: VisibilityLevel::from_ast(field.visibility()),
                    ty: field
                        .ty()
                        .map(|ty| TypeRef::from_ast(ty, line_index, interner))
                        .unwrap_or_else(|| TypeRef::unknown_from_text(normalized_syntax(&field))),
                    span: Span::from_text_range(span),
                    docs: Documentation::from_ast(&field),
                }
            })
            .collect()
    }

    fn tuple_list_from_ast(
        fields: ast::TupleFieldList,
        line_index: &LineIndex,
        interner: &mut NameInterner,
    ) -> Vec<Self> {
        fields
            .fields()
            .enumerate()
            .map(|(index, field)| Self {
                key: Some(FieldKey::Tuple(index)),
                visibility: VisibilityLevel::from_ast(field.visibility()),
                ty: field
                    .ty()
                    .map(|ty| TypeRef::from_ast(ty, line_index, interner))
                    .unwrap_or_else(|| TypeRef::unknown_from_text(normalized_syntax(&field))),
                span: Span::from_text_range(field.syntax().text_range()),
                docs: Documentation::from_ast(&field),
            })
            .collect()
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

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct TraitItem {
    pub generics: GenericParams,
    pub super_traits: Vec<TypeBound>,
    pub items: Vec<ItemTreeId>,
    pub is_unsafe: bool,
}

impl TraitItem {
    pub fn from_ast(
        item: &ast::Trait,
        items: Vec<ItemTreeId>,
        line_index: &LineIndex,
        interner: &mut NameInterner,
    ) -> Self {
        Self {
            generics: GenericParams::from_ast(item, line_index, interner),
            super_traits: TypeBound::list_from_ast(item.type_bound_list(), line_index, interner),
            items,
            is_unsafe: item.unsafe_token().is_some(),
        }
    }

    pub fn shrink_to_fit(&mut self) {
        self.generics.shrink_to_fit();
        self.super_traits.shrink_to_fit();
        for bound in &mut self.super_traits {
            bound.shrink_to_fit();
        }
        self.items.shrink_to_fit();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ImplItem {
    pub generics: GenericParams,
    pub trait_ref: Option<TypeRef>,
    pub self_ty: TypeRef,
    pub items: Vec<ItemTreeId>,
    pub is_unsafe: bool,
}

impl ImplItem {
    pub fn from_ast(
        item: &ast::Impl,
        items: Vec<ItemTreeId>,
        line_index: &LineIndex,
        interner: &mut NameInterner,
    ) -> Self {
        let (trait_ref, self_ty) = Self::header_from_ast(item, line_index, interner);

        Self {
            generics: GenericParams::from_ast(item, line_index, interner),
            trait_ref,
            self_ty,
            items,
            is_unsafe: item.unsafe_token().is_some(),
        }
    }

    fn header_from_ast(
        item: &ast::Impl,
        line_index: &LineIndex,
        interner: &mut NameInterner,
    ) -> (Option<TypeRef>, TypeRef) {
        // `ra_syntax` exposes impl headers as child type nodes. The presence of `for` decides
        // whether the first type is a trait path or the inherent self type.
        let types = item
            .syntax()
            .children()
            .filter_map(ast::Type::cast)
            .collect::<Vec<_>>();

        if item.for_token().is_some() {
            let trait_ref = types
                .first()
                .cloned()
                .map(|ty| TypeRef::from_ast(ty, line_index, interner));
            let self_ty = types
                .get(1)
                .cloned()
                .map(|ty| TypeRef::from_ast(ty, line_index, interner))
                .unwrap_or_else(|| TypeRef::unknown_from_text(normalized_syntax(item)));
            return (trait_ref, self_ty);
        }

        let self_ty = types
            .first()
            .cloned()
            .map(|ty| TypeRef::from_ast(ty, line_index, interner))
            .unwrap_or_else(|| TypeRef::unknown_from_text(normalized_syntax(item)));
        (None, self_ty)
    }

    pub fn shrink_to_fit(&mut self) {
        self.generics.shrink_to_fit();
        if let Some(trait_ref) = &mut self.trait_ref {
            trait_ref.shrink_to_fit();
        }
        self.self_ty.shrink_to_fit();
        self.items.shrink_to_fit();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct TypeAliasItem {
    pub generics: GenericParams,
    pub bounds: Vec<TypeBound>,
    pub aliased_ty: Option<TypeRef>,
}

impl TypeAliasItem {
    pub fn from_ast(
        item: &ast::TypeAlias,
        line_index: &LineIndex,
        interner: &mut NameInterner,
    ) -> Self {
        Self {
            generics: GenericParams::from_ast(item, line_index, interner),
            bounds: TypeBound::list_from_ast(item.type_bound_list(), line_index, interner),
            aliased_ty: item
                .ty()
                .map(|ty| TypeRef::from_ast(ty, line_index, interner)),
        }
    }

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

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ConstItem {
    pub generics: GenericParams,
    pub ty: Option<TypeRef>,
}

impl ConstItem {
    pub fn from_ast(
        item: &ast::Const,
        line_index: &LineIndex,
        interner: &mut NameInterner,
    ) -> Self {
        Self {
            generics: GenericParams::from_ast(item, line_index, interner),
            ty: item
                .ty()
                .map(|ty| TypeRef::from_ast(ty, line_index, interner)),
        }
    }

    pub fn shrink_to_fit(&mut self) {
        self.generics.shrink_to_fit();
        if let Some(ty) = &mut self.ty {
            ty.shrink_to_fit();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct StaticItem {
    pub ty: Option<TypeRef>,
    pub mutability: Mutability,
}

impl StaticItem {
    pub fn from_ast(
        item: &ast::Static,
        line_index: &LineIndex,
        interner: &mut NameInterner,
    ) -> Self {
        Self {
            ty: item
                .ty()
                .map(|ty| TypeRef::from_ast(ty, line_index, interner)),
            mutability: Mutability::from_mut_token(item.mut_token().is_some()),
        }
    }

    pub fn shrink_to_fit(&mut self) {
        if let Some(ty) = &mut self.ty {
            ty.shrink_to_fit();
        }
    }
}

fn lifetime_bounds_from_ast(bound_list: Option<ast::TypeBoundList>) -> Vec<String> {
    bound_list
        .into_iter()
        .flat_map(|bound_list| bound_list.bounds())
        .filter_map(|bound| {
            bound
                .lifetime()
                .map(|lifetime| normalized_syntax(&lifetime))
        })
        .collect()
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
