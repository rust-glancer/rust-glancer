use rg_ir_model::{
    Mutability,
    items::{
        ConstItem, ConstParamData, Documentation, EnumItem, EnumVariantItem, FieldItem, FieldKey,
        FieldList, FunctionItem, FunctionQualifiers, GenericParams, ImplItem, ItemTreeId,
        LifetimeParamData, ParamItem, ParamKind, StaticItem, StructItem, TraitItem, TypeAliasItem,
        TypeParamData, TypeRef, UnionItem, VisibilityLevel, WherePredicate,
    },
};
use rg_parse::{LineIndex, Span};
use rg_syntax::{
    AstNode as _,
    ast::{self, HasGenericParams, HasName, HasTypeBounds, HasVisibility},
};
use rg_text::NameInterner;

use super::{FromAst, MaybeFromAst, OuterDocs, normalized_syntax, type_bound_list_from_ast};

pub struct TraitItemContext<'a> {
    pub items: Vec<ItemTreeId>,
    pub line_index: &'a LineIndex,
    pub interner: &'a mut NameInterner,
}

pub struct ImplItemContext<'a> {
    pub items: Vec<ItemTreeId>,
    pub line_index: &'a LineIndex,
    pub interner: &'a mut NameInterner,
}

impl FromAst for GenericParams {
    type AstNode = dyn HasGenericParams;
    type Context<'a> = (&'a LineIndex, &'a mut NameInterner);

    fn from_ast(item: &Self::AstNode, (line_index, interner): Self::Context<'_>) -> Self {
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
                                .map(|ty| TypeRef::from_ast(&ty, (line_index, &mut *interner))),
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
                            bounds: type_bound_list_from_ast(
                                param.type_bound_list(),
                                line_index,
                                &mut *interner,
                            ),
                            default: param
                                .default_type()
                                .map(|ty| TypeRef::from_ast(&ty, (line_index, &mut *interner))),
                        });
                    }
                }
            }
        }

        if let Some(where_clause) = item.where_clause() {
            params.where_predicates = where_clause
                .predicates()
                .map(|predicate| WherePredicate::from_ast(&predicate, (line_index, &mut *interner)))
                .collect();
        }

        params
    }
}

impl FromAst for WherePredicate {
    type AstNode = ast::WherePred;
    type Context<'a> = (&'a LineIndex, &'a mut NameInterner);

    fn from_ast(predicate: &Self::AstNode, (line_index, interner): Self::Context<'_>) -> Self {
        if let Some(lifetime) = predicate.lifetime() {
            return Self::Lifetime {
                lifetime: normalized_syntax(&lifetime),
                bounds: lifetime_bounds_from_ast(predicate.type_bound_list()),
            };
        }

        if let Some(ty) = predicate.ty() {
            return Self::Type {
                ty: TypeRef::from_ast(&ty, (line_index, &mut *interner)),
                bounds: type_bound_list_from_ast(predicate.type_bound_list(), line_index, interner),
            };
        }

        Self::Unsupported(normalized_syntax(predicate))
    }
}

impl FromAst for FunctionItem {
    type AstNode = ast::Fn;
    type Context<'a> = (&'a LineIndex, &'a mut NameInterner);

    fn from_ast(item: &Self::AstNode, (line_index, interner): Self::Context<'_>) -> Self {
        Self {
            generics: GenericParams::from_ast(item, (line_index, &mut *interner)),
            params: param_list_from_ast(item.param_list(), line_index, &mut *interner),
            ret_ty: item
                .ret_type()
                .and_then(|ret_ty| ret_ty.ty())
                .map(|ty| TypeRef::from_ast(&ty, (line_index, &mut *interner))),
            qualifiers: FunctionQualifiers {
                is_async: item.async_token().is_some(),
                is_const: item.const_token().is_some(),
                is_unsafe: item.unsafe_token().is_some(),
            },
        }
    }
}

impl FromAst for StructItem {
    type AstNode = ast::Struct;
    type Context<'a> = (&'a LineIndex, &'a mut NameInterner);

    fn from_ast(item: &Self::AstNode, (line_index, interner): Self::Context<'_>) -> Self {
        Self {
            generics: GenericParams::from_ast(item, (line_index, &mut *interner)),
            fields: FieldList::from_ast(&item.field_list(), (line_index, interner)),
        }
    }
}

impl FromAst for UnionItem {
    type AstNode = ast::Union;
    type Context<'a> = (&'a LineIndex, &'a mut NameInterner);

    fn from_ast(item: &Self::AstNode, (line_index, interner): Self::Context<'_>) -> Self {
        Self {
            generics: GenericParams::from_ast(item, (line_index, &mut *interner)),
            fields: item
                .record_field_list()
                .map(|fields| record_field_list_from_ast(&fields, line_index, &mut *interner))
                .unwrap_or_default(),
        }
    }
}

impl FromAst for EnumItem {
    type AstNode = ast::Enum;
    type Context<'a> = (&'a LineIndex, &'a mut NameInterner);

    fn from_ast(item: &Self::AstNode, (line_index, interner): Self::Context<'_>) -> Self {
        Self {
            generics: GenericParams::from_ast(item, (line_index, &mut *interner)),
            variants: item
                .variant_list()
                .map(|variant_list| {
                    variant_list
                        .variants()
                        .map(|variant| {
                            EnumVariantItem::from_ast(&variant, (line_index, &mut *interner))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

impl FromAst for EnumVariantItem {
    type AstNode = ast::Variant;
    type Context<'a> = (&'a LineIndex, &'a mut NameInterner);

    fn from_ast(variant: &Self::AstNode, (line_index, interner): Self::Context<'_>) -> Self {
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
            docs: <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(variant, OuterDocs),
            fields: FieldList::from_ast(&variant.field_list(), (line_index, interner)),
        }
    }
}

impl FromAst for FieldList {
    type AstNode = Option<ast::FieldList>;
    type Context<'a> = (&'a LineIndex, &'a mut NameInterner);

    fn from_ast(field_list: &Self::AstNode, (line_index, interner): Self::Context<'_>) -> Self {
        match field_list {
            Some(ast::FieldList::RecordFieldList(fields)) => {
                Self::Named(record_field_list_from_ast(fields, line_index, interner))
            }
            Some(ast::FieldList::TupleFieldList(fields)) => {
                Self::Tuple(tuple_field_list_from_ast(fields, line_index, interner))
            }
            None => Self::Unit,
        }
    }
}

impl FromAst for TraitItem {
    type AstNode = ast::Trait;
    type Context<'a> = TraitItemContext<'a>;

    fn from_ast(item: &Self::AstNode, ctx: Self::Context<'_>) -> Self {
        Self {
            generics: GenericParams::from_ast(item, (ctx.line_index, &mut *ctx.interner)),
            super_traits: type_bound_list_from_ast(
                item.type_bound_list(),
                ctx.line_index,
                ctx.interner,
            ),
            items: ctx.items,
            is_unsafe: item.unsafe_token().is_some(),
        }
    }
}

impl FromAst for ImplItem {
    type AstNode = ast::Impl;
    type Context<'a> = ImplItemContext<'a>;

    fn from_ast(item: &Self::AstNode, ctx: Self::Context<'_>) -> Self {
        let (trait_ref, self_ty) = impl_header_from_ast(item, ctx.line_index, &mut *ctx.interner);

        Self {
            generics: GenericParams::from_ast(item, (ctx.line_index, &mut *ctx.interner)),
            trait_ref,
            self_ty,
            items: ctx.items,
            is_unsafe: item.unsafe_token().is_some(),
        }
    }
}

impl FromAst for TypeAliasItem {
    type AstNode = ast::TypeAlias;
    type Context<'a> = (&'a LineIndex, &'a mut NameInterner);

    fn from_ast(item: &Self::AstNode, (line_index, interner): Self::Context<'_>) -> Self {
        Self {
            generics: GenericParams::from_ast(item, (line_index, &mut *interner)),
            bounds: type_bound_list_from_ast(item.type_bound_list(), line_index, &mut *interner),
            aliased_ty: item
                .ty()
                .map(|ty| TypeRef::from_ast(&ty, (line_index, &mut *interner))),
        }
    }
}

impl FromAst for ConstItem {
    type AstNode = ast::Const;
    type Context<'a> = (&'a LineIndex, &'a mut NameInterner);

    fn from_ast(item: &Self::AstNode, (line_index, interner): Self::Context<'_>) -> Self {
        Self {
            generics: GenericParams::from_ast(item, (line_index, &mut *interner)),
            ty: item
                .ty()
                .map(|ty| TypeRef::from_ast(&ty, (line_index, &mut *interner))),
        }
    }
}

impl FromAst for StaticItem {
    type AstNode = ast::Static;
    type Context<'a> = (&'a LineIndex, &'a mut NameInterner);

    fn from_ast(item: &Self::AstNode, (line_index, interner): Self::Context<'_>) -> Self {
        Self {
            ty: item
                .ty()
                .map(|ty| TypeRef::from_ast(&ty, (line_index, interner))),
            mutability: Mutability::from_mut_token(item.mut_token().is_some()),
        }
    }
}

fn param_list_from_ast(
    param_list: Option<ast::ParamList>,
    line_index: &LineIndex,
    interner: &mut NameInterner,
) -> Vec<ParamItem> {
    let Some(param_list) = param_list else {
        return Vec::new();
    };

    let mut params = Vec::new();

    if let Some(self_param) = param_list.self_param() {
        params.push(ParamItem {
            pat: normalized_syntax(&self_param),
            ty: self_param
                .ty()
                .map(|ty| TypeRef::from_ast(&ty, (line_index, &mut *interner))),
            kind: ParamKind::SelfParam,
        });
    }

    for param in param_list.params() {
        params.push(ParamItem {
            pat: param
                .pat()
                .map(|pat| normalized_syntax(&pat))
                .unwrap_or_else(|| "<missing>".to_string()),
            ty: param
                .ty()
                .map(|ty| TypeRef::from_ast(&ty, (line_index, &mut *interner))),
            kind: ParamKind::Normal,
        });
    }

    params
}

fn record_field_list_from_ast(
    fields: &ast::RecordFieldList,
    line_index: &LineIndex,
    interner: &mut NameInterner,
) -> Vec<FieldItem> {
    fields
        .fields()
        .map(|field| {
            let name = field.name();
            let span = name
                .as_ref()
                .map(|name| name.syntax().text_range())
                .unwrap_or_else(|| field.syntax().text_range());

            FieldItem {
                key: name.map(|name| FieldKey::Named(interner.intern(name.text()))),
                visibility: VisibilityLevel::from_ast(&field.visibility(), ()),
                ty: field
                    .ty()
                    .map(|ty| TypeRef::from_ast(&ty, (line_index, &mut *interner)))
                    .unwrap_or_else(|| TypeRef::unknown_from_text(normalized_syntax(&field))),
                span: Span::from_text_range(span),
                docs: <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(&field, OuterDocs),
            }
        })
        .collect()
}

fn tuple_field_list_from_ast(
    fields: &ast::TupleFieldList,
    line_index: &LineIndex,
    interner: &mut NameInterner,
) -> Vec<FieldItem> {
    fields
        .fields()
        .enumerate()
        .map(|(index, field)| FieldItem {
            key: Some(FieldKey::Tuple(index)),
            visibility: VisibilityLevel::from_ast(&field.visibility(), ()),
            ty: field
                .ty()
                .map(|ty| TypeRef::from_ast(&ty, (line_index, &mut *interner)))
                .unwrap_or_else(|| TypeRef::unknown_from_text(normalized_syntax(&field))),
            span: Span::from_text_range(field.syntax().text_range()),
            docs: <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(&field, OuterDocs),
        })
        .collect()
}

fn impl_header_from_ast(
    item: &ast::Impl,
    line_index: &LineIndex,
    interner: &mut NameInterner,
) -> (Option<TypeRef>, TypeRef) {
    // `rg_syntax` exposes impl headers as child type nodes. The presence of `for` decides whether
    // the first type is a trait path or the inherent self type.
    let types = item
        .syntax()
        .children()
        .filter_map(ast::Type::cast)
        .collect::<Vec<_>>();

    if item.for_token().is_some() {
        let trait_ref = types
            .first()
            .cloned()
            .map(|ty| TypeRef::from_ast(&ty, (line_index, &mut *interner)));
        let self_ty = types
            .get(1)
            .cloned()
            .map(|ty| TypeRef::from_ast(&ty, (line_index, &mut *interner)))
            .unwrap_or_else(|| TypeRef::unknown_from_text(normalized_syntax(item)));
        return (trait_ref, self_ty);
    }

    let self_ty = types
        .first()
        .cloned()
        .map(|ty| TypeRef::from_ast(&ty, (line_index, interner)))
        .unwrap_or_else(|| TypeRef::unknown_from_text(normalized_syntax(item)));
    (None, self_ty)
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
