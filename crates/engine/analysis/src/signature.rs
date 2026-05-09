//! Compact Rust-ish declaration labels for hover and related UI surfaces.
//!
//! The renderer deliberately stays syntactic. It formats the declaration facts our IR already
//! stores instead of trying to reconstruct rustc-perfect signatures.

use rg_body_ir::{BindingData, BodyFieldData, BodyFunctionData, BodyItemData, BodyTy};
use rg_semantic_ir::{
    ConstData, EnumData, EnumVariantData, EnumVariantItem, FieldData, FieldItem, FieldKey,
    FieldList, FunctionData, FunctionItem, FunctionQualifiers, GenericParams, Mutability,
    ParamItem, StaticData, StructData, TraitData, TypeAliasData, TypeBound, TypeRef, UnionData,
    VisibilityLevel, WherePredicate,
};

use super::{Analysis, type_render::TypeRenderer};

const MEMBER_PREVIEW_LIMIT: usize = 5;

pub(super) struct SignatureRenderer<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> SignatureRenderer<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(super) fn struct_signature(&self, data: &StructData) -> String {
        let header = format!(
            "{}struct {}{}{}",
            visibility_prefix(&data.visibility),
            data.name,
            generic_params(&data.generics),
            where_clause(&data.generics)
        );
        item_with_fields(header, &data.fields)
    }

    pub(super) fn union_signature(&self, data: &UnionData) -> String {
        let header = format!(
            "{}union {}{}{}",
            visibility_prefix(&data.visibility),
            data.name,
            generic_params(&data.generics),
            where_clause(&data.generics)
        );
        item_with_record_fields(header, &data.fields)
    }

    pub(super) fn enum_signature(&self, data: &EnumData) -> String {
        let header = format!(
            "{}enum {}{}{}",
            visibility_prefix(&data.visibility),
            data.name,
            generic_params(&data.generics),
            where_clause(&data.generics)
        );
        if data.variants.is_empty() {
            return format!("{header} {{}}");
        }

        format_block(header, data.variants.iter().map(enum_variant_signature))
    }

    pub(super) fn trait_signature(&self, data: &TraitData) -> String {
        let unsafe_prefix = if data.is_unsafe { "unsafe " } else { "" };
        let super_traits = if data.super_traits.is_empty() {
            String::new()
        } else {
            format!(
                ": {}",
                data.super_traits
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" + ")
            )
        };
        format!(
            "{}{unsafe_prefix}trait {}{}{}{}",
            visibility_prefix(&data.visibility),
            data.name,
            generic_params(&data.generics),
            super_traits,
            where_clause(&data.generics)
        )
    }

    pub(super) fn function_signature(&self, data: &FunctionData) -> String {
        format!(
            "{}{}",
            visibility_prefix(&data.visibility),
            function_signature_from_parts(
                &data.name,
                data.signature.generics(),
                data.signature.params(),
                data.signature.ret_ty(),
                data.signature.qualifiers(),
            )
        )
    }

    pub(super) fn type_alias_signature(&self, data: &TypeAliasData) -> String {
        format!(
            "{}{}",
            visibility_prefix(&data.visibility),
            type_alias_signature(
                &data.name,
                data.signature.generics(),
                data.signature.bounds(),
                data.signature.aliased_ty(),
            )
        )
    }

    pub(super) fn const_signature(&self, data: &ConstData) -> String {
        format!(
            "{}{}",
            visibility_prefix(&data.visibility),
            const_signature(&data.name, data.signature.ty())
        )
    }

    pub(super) fn static_signature(&self, data: &StaticData) -> String {
        format!(
            "{}{}",
            visibility_prefix(&data.visibility),
            static_signature(&data.name, data.mutability, data.ty.as_ref())
        )
    }

    pub(super) fn field_signature(&self, data: FieldData<'_>) -> Option<String> {
        field_signature(data.field)
    }

    pub(super) fn enum_variant_signature(&self, data: EnumVariantData<'_>) -> String {
        enum_variant_signature(data.variant)
    }

    pub(super) fn local_item_signature(&self, data: &BodyItemData) -> String {
        let header = format!(
            "{} {}{}{}",
            data.kind,
            data.name,
            generic_params(&data.generics),
            where_clause(&data.generics)
        );
        item_with_fields(header, &data.fields)
    }

    pub(super) fn local_function_signature(&self, data: &BodyFunctionData) -> String {
        function_signature(&data.name, &data.declaration)
    }

    pub(super) fn local_field_signature(&self, data: BodyFieldData<'_>) -> Option<String> {
        field_signature(data.field)
    }

    pub(super) fn binding_signature(&self, data: &BindingData) -> anyhow::Result<String> {
        let name = data.name.as_deref().unwrap_or("<unsupported>");
        let ty = TypeRenderer::new(self.0)
            .render(&data.ty)?
            .or_else(|| data.annotation.as_ref().map(ToString::to_string))
            .unwrap_or_else(|| "_".to_string());

        Ok(format!("let {name}: {ty}"))
    }

    pub(super) fn ty_signature(&self, ty: &BodyTy) -> anyhow::Result<Option<String>> {
        TypeRenderer::new(self.0).render(ty)
    }
}

fn visibility_prefix(visibility: &VisibilityLevel) -> String {
    if matches!(visibility, VisibilityLevel::Private) {
        String::new()
    } else {
        format!("{visibility} ")
    }
}

fn function_signature(name: &str, item: &FunctionItem) -> String {
    function_signature_from_parts(
        name,
        Some(&item.generics),
        &item.params,
        item.ret_ty.as_ref(),
        item.qualifiers,
    )
}

fn function_signature_from_parts(
    name: &str,
    generics: Option<&GenericParams>,
    params: &[ParamItem],
    ret_ty: Option<&TypeRef>,
    qualifiers: FunctionQualifiers,
) -> String {
    let mut signature = String::new();
    if qualifiers.is_const {
        signature.push_str("const ");
    }
    if qualifiers.is_unsafe {
        signature.push_str("unsafe ");
    }
    if qualifiers.is_async {
        signature.push_str("async ");
    }

    signature.push_str("fn ");
    signature.push_str(name);
    signature.push_str(&generic_params_opt(generics));
    signature.push('(');
    signature.push_str(
        &params
            .iter()
            .map(param_signature)
            .collect::<Vec<_>>()
            .join(", "),
    );
    signature.push(')');
    if let Some(ret_ty) = ret_ty
        && !matches!(ret_ty, TypeRef::Unit)
    {
        signature.push_str(" -> ");
        signature.push_str(&ret_ty.to_string());
    }
    signature.push_str(&where_clause_opt(generics));

    signature
}

fn param_signature(param: &ParamItem) -> String {
    match &param.ty {
        Some(ty) => format!("{}: {ty}", param.pat),
        None => param.pat.clone(),
    }
}

fn type_alias_signature(
    name: &str,
    generics: Option<&GenericParams>,
    bounds: &[TypeBound],
    aliased_ty: Option<&TypeRef>,
) -> String {
    let bounds = if bounds.is_empty() {
        String::new()
    } else {
        format!(": {}", type_bounds(bounds))
    };
    let mut signature = format!(
        "type {name}{}{}{}",
        generic_params_opt(generics),
        bounds,
        where_clause_opt(generics),
    );
    if let Some(ty) = aliased_ty {
        signature.push_str(" = ");
        signature.push_str(&ty.to_string());
    }
    signature
}

fn const_signature(name: &str, ty: Option<&TypeRef>) -> String {
    match ty {
        Some(ty) => format!("const {name}: {ty}"),
        None => format!("const {name}: _"),
    }
}

fn static_signature(name: &str, mutability: Mutability, ty: Option<&TypeRef>) -> String {
    let mut_prefix = matches!(mutability, Mutability::Mutable)
        .then_some("mut ")
        .unwrap_or_default();
    match ty {
        Some(ty) => format!("static {mut_prefix}{name}: {ty}"),
        None => format!("static {mut_prefix}{name}: _"),
    }
}

fn item_with_fields(header: String, fields: &FieldList) -> String {
    match fields {
        FieldList::Named(fields) => item_with_record_fields(header, fields),
        FieldList::Tuple(fields) => item_with_tuple_fields(header, fields),
        FieldList::Unit => header,
    }
}

fn item_with_record_fields(header: String, fields: &[FieldItem]) -> String {
    if fields.is_empty() {
        return format!("{header} {{}}");
    }

    format_block(header, fields.iter().map(record_field_signature))
}

fn item_with_tuple_fields(header: String, fields: &[FieldItem]) -> String {
    let mut rendered = fields
        .iter()
        .take(MEMBER_PREVIEW_LIMIT)
        .map(tuple_field_signature)
        .collect::<Vec<_>>();
    if fields.len() > MEMBER_PREVIEW_LIMIT {
        rendered.push("...".to_string());
    }

    format!("{header}({});", rendered.join(", "))
}

fn enum_variant_signature(variant: &EnumVariantItem) -> String {
    match &variant.fields {
        FieldList::Named(fields) if fields.is_empty() => format!("{} {{}}", variant.name),
        FieldList::Named(fields) => {
            let rendered =
                capped_inline_rows(fields.iter().map(record_field_signature), fields.len());
            format!("{} {{ {} }}", variant.name, rendered.join(", "))
        }
        FieldList::Tuple(fields) => {
            let rendered =
                capped_inline_rows(fields.iter().map(tuple_field_signature), fields.len());
            format!("{}({})", variant.name, rendered.join(", "))
        }
        FieldList::Unit => variant.name.to_string(),
    }
}

fn capped_inline_rows(rows: impl Iterator<Item = String>, total_len: usize) -> Vec<String> {
    let mut rendered = rows.take(MEMBER_PREVIEW_LIMIT).collect::<Vec<_>>();
    if total_len > MEMBER_PREVIEW_LIMIT {
        rendered.push("...".to_string());
    }
    rendered
}

fn format_block(header: String, rows: impl Iterator<Item = String>) -> String {
    let mut rendered = rows.take(MEMBER_PREVIEW_LIMIT + 1).collect::<Vec<_>>();
    let truncated = rendered.len() > MEMBER_PREVIEW_LIMIT;
    rendered.truncate(MEMBER_PREVIEW_LIMIT);
    if truncated {
        rendered.push("...".to_string());
    }

    let body = rendered
        .into_iter()
        .map(|row| format!("    {row},"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{header} {{\n{body}\n}}")
}

fn record_field_signature(field: &FieldItem) -> String {
    field_signature(field).unwrap_or_else(|| {
        format!(
            "{}<missing>: {}",
            visibility_prefix(&field.visibility),
            field.ty
        )
    })
}

fn tuple_field_signature(field: &FieldItem) -> String {
    format!("{}{}", visibility_prefix(&field.visibility), field.ty)
}

fn field_signature(field: &FieldItem) -> Option<String> {
    let key = field.key.as_ref()?;
    let label = match key {
        FieldKey::Named(name) => name.to_string(),
        FieldKey::Tuple(index) => index.to_string(),
    };
    Some(format!(
        "{}{}: {}",
        visibility_prefix(&field.visibility),
        label,
        field.ty
    ))
}

fn generic_params(generics: &GenericParams) -> String {
    let mut params = Vec::new();

    params.extend(generics.lifetimes.iter().map(|param| {
        if param.bounds.is_empty() {
            param.name.to_string()
        } else {
            format!("{}: {}", param.name, param.bounds.join(" + "))
        }
    }));
    params.extend(generics.types.iter().map(|param| {
        let mut text = param.name.to_string();
        if !param.bounds.is_empty() {
            text.push_str(": ");
            text.push_str(&type_bounds(&param.bounds));
        }
        if let Some(default) = &param.default {
            text.push_str(" = ");
            text.push_str(&default.to_string());
        }
        text
    }));
    params.extend(generics.consts.iter().map(|param| {
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

    if params.is_empty() {
        String::new()
    } else {
        format!("<{}>", params.join(", "))
    }
}

fn generic_params_opt(generics: Option<&GenericParams>) -> String {
    generics.map(generic_params).unwrap_or_default()
}

fn where_clause(generics: &GenericParams) -> String {
    if generics.where_predicates.is_empty() {
        return String::new();
    }

    format!(
        " where {}",
        generics
            .where_predicates
            .iter()
            .map(where_predicate)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn where_clause_opt(generics: Option<&GenericParams>) -> String {
    generics.map(where_clause).unwrap_or_default()
}

fn where_predicate(predicate: &WherePredicate) -> String {
    match predicate {
        WherePredicate::Type { ty, bounds } => {
            if bounds.is_empty() {
                ty.to_string()
            } else {
                format!("{ty}: {}", type_bounds(bounds))
            }
        }
        WherePredicate::Lifetime { lifetime, bounds } => {
            format!("{lifetime}: {}", bounds.join(" + "))
        }
        WherePredicate::Unsupported(text) => format!("<unsupported:{text}>"),
    }
}

fn type_bounds(bounds: &[TypeBound]) -> String {
    bounds
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" + ")
}
