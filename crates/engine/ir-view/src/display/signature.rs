//! Compact Rust-ish declaration labels for hover and related UI surfaces.
//!
//! The renderer deliberately stays syntactic. It formats the declaration facts our IR already
//! stores instead of trying to reconstruct rustc-perfect signatures. Canonical semantic names are
//! passed through `SyntaxRenderer` so every emitted identifier is valid for the use-site edition.

use std::fmt::Write as _;

use rg_body_ir::BindingData;
use rg_ir_model::Mutability;
use rg_ir_model::items::{
    EnumVariantItem, FieldItem, FieldList, FunctionQualifiers, GenericParams, ParamItem,
    TypeOrConstParamData, TypeRef, VisibilityLevel, WherePredicate,
};
use rg_semantic_ir::{
    ConstData, EnumData, FieldData, FunctionData, StaticData, StructData, TraitData, TypeAliasData,
    UnionData,
};
use rg_text::RustEdition;
use rg_ty::Ty;

use crate::{
    IndexedViewDb,
    display::{syntax::SyntaxRenderer, ty_label::TypeRenderer},
};

const MEMBER_PREVIEW_LIMIT: usize = 5;

/// Renders compact Rust-like declaration signatures for one edition.
#[derive(Debug, Clone, Copy)]
pub struct SignatureRenderer {
    syntax: SyntaxRenderer,
}

impl SignatureRenderer {
    pub fn new(edition: RustEdition) -> Self {
        Self {
            syntax: SyntaxRenderer::new(edition),
        }
    }

    /// Render a struct signature.
    pub fn struct_signature(&self, data: &StructData) -> String {
        let header = format!(
            "{}struct {}{}{}",
            Self::visibility_prefix(&data.visibility),
            self.syntax.identifier(&data.name),
            self.generic_params(&data.generics),
            self.where_clause(&data.generics)
        );
        self.item_with_fields(header, &data.fields)
    }

    /// Render a union signature.
    pub fn union_signature(&self, data: &UnionData) -> String {
        let header = format!(
            "{}union {}{}{}",
            Self::visibility_prefix(&data.visibility),
            self.syntax.identifier(&data.name),
            self.generic_params(&data.generics),
            self.where_clause(&data.generics)
        );
        self.item_with_record_fields(header, &data.fields)
    }

    /// Render an enum signature.
    pub fn enum_signature(&self, data: &EnumData) -> String {
        let header = format!(
            "{}enum {}{}{}",
            Self::visibility_prefix(&data.visibility),
            self.syntax.identifier(&data.name),
            self.generic_params(&data.generics),
            self.where_clause(&data.generics)
        );
        if data.variants.is_empty() {
            return format!("{header} {{}}");
        }

        Self::format_block(
            header,
            data.variants
                .iter()
                .map(|variant| self.enum_variant_signature(variant)),
        )
    }

    /// Render a trait signature.
    pub fn trait_signature(&self, data: &TraitData) -> String {
        let unsafe_prefix = if data.is_unsafe { "unsafe " } else { "" };
        let super_traits = if data.super_traits.is_empty() {
            String::new()
        } else {
            format!(": {}", self.syntax.type_bounds(&data.super_traits))
        };
        format!(
            "{}{unsafe_prefix}trait {}{}{}{}",
            Self::visibility_prefix(&data.visibility),
            self.syntax.identifier(&data.name),
            self.generic_params(&data.generics),
            super_traits,
            self.where_clause(&data.generics)
        )
    }

    /// Render a function or method signature.
    pub fn function_signature(&self, data: &FunctionData) -> String {
        format!(
            "{}{}",
            Self::visibility_prefix(&data.visibility),
            self.function_signature_from_parts(
                &data.name,
                data.signature.generics(),
                data.signature.params(),
                data.signature.ret_ty(),
                data.signature.qualifiers(),
            )
        )
    }

    /// Render a type alias signature.
    pub fn type_alias_signature(&self, data: &TypeAliasData) -> String {
        let bounds = if data.signature.bounds().is_empty() {
            String::new()
        } else {
            format!(": {}", self.syntax.type_bounds(data.signature.bounds()))
        };
        let mut signature = format!(
            "{}type {}{}{}{}",
            Self::visibility_prefix(&data.visibility),
            self.syntax.identifier(&data.name),
            self.generic_params_opt(data.signature.generics()),
            bounds,
            self.where_clause_opt(data.signature.generics()),
        );
        if let Some(ty) = data.signature.aliased_ty() {
            signature.push_str(" = ");
            write!(signature, "{}", self.syntax.type_ref(ty))
                .expect("string writes should not fail");
        }
        signature
    }

    /// Render a const signature.
    pub fn const_signature(&self, data: &ConstData) -> String {
        let visibility = Self::visibility_prefix(&data.visibility);
        let name = self.syntax.identifier(&data.name);
        match data.signature.ty() {
            Some(ty) => format!("{visibility}const {name}: {}", self.syntax.type_ref(ty)),
            None => format!("{visibility}const {name}: _"),
        }
    }

    /// Render a static signature.
    pub fn static_signature(&self, data: &StaticData) -> String {
        let visibility = Self::visibility_prefix(&data.visibility);
        let mut_prefix = matches!(data.mutability, Mutability::Mutable)
            .then_some("mut ")
            .unwrap_or_default();
        let name = self.syntax.identifier(&data.name);
        match data.ty.as_ref() {
            Some(ty) => format!(
                "{visibility}static {mut_prefix}{name}: {}",
                self.syntax.type_ref(ty)
            ),
            None => format!("{visibility}static {mut_prefix}{name}: _"),
        }
    }

    /// Render a field signature.
    pub fn field_signature(&self, data: FieldData<'_>) -> Option<String> {
        self.field_item_signature(data.field)
    }

    /// Render an enum variant signature.
    pub fn enum_variant_signature(&self, variant: &EnumVariantItem) -> String {
        let name = self.syntax.identifier(&variant.name);
        match &variant.fields {
            FieldList::Named(fields) if fields.is_empty() => format!("{name} {{}}"),
            FieldList::Named(fields) => {
                let rendered = Self::capped_inline_rows(
                    fields
                        .iter()
                        .map(|field| self.record_field_signature(field)),
                    fields.len(),
                );
                format!("{name} {{ {} }}", rendered.join(", "))
            }
            FieldList::Tuple(fields) => {
                let rendered = Self::capped_inline_rows(
                    fields.iter().map(|field| self.tuple_field_signature(field)),
                    fields.len(),
                );
                format!("{name}({})", rendered.join(", "))
            }
            FieldList::Unit => name.to_string(),
        }
    }

    /// Render a body binding signature.
    pub fn binding_signature(
        &self,
        db: &IndexedViewDb<'_>,
        data: &BindingData,
        ty: Option<&Ty>,
    ) -> anyhow::Result<String> {
        let rendered_ty = ty
            .map(|ty| TypeRenderer::new(db, self.syntax.edition()).render(ty))
            .transpose()?
            .flatten();

        let mut signature = "let ".to_string();
        match data.name.as_deref() {
            Some(name) => write!(signature, "{}", self.syntax.identifier(name))
                .expect("string writes should not fail"),
            None => signature.push_str("<unsupported>"),
        }
        signature.push_str(": ");
        match (rendered_ty, data.annotation.as_ref()) {
            (Some(ty), _) => signature.push_str(&ty),
            (None, Some(ty)) => write!(signature, "{}", self.syntax.type_ref(ty))
                .expect("string writes should not fail"),
            (None, None) => signature.push('_'),
        }

        Ok(signature)
    }

    fn visibility_prefix(visibility: &VisibilityLevel) -> String {
        if matches!(visibility, VisibilityLevel::Private) {
            String::new()
        } else {
            format!("{visibility} ")
        }
    }

    fn function_signature_from_parts(
        &self,
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
        write!(signature, "{}", self.syntax.identifier(name))
            .expect("string writes should not fail");
        signature.push_str(&self.generic_params_opt(generics));
        signature.push('(');
        signature.push_str(
            &params
                .iter()
                .map(|param| self.param_signature(param))
                .collect::<Vec<_>>()
                .join(", "),
        );
        signature.push(')');
        if let Some(ret_ty) = ret_ty
            && !matches!(ret_ty, TypeRef::Unit)
        {
            signature.push_str(" -> ");
            write!(signature, "{}", self.syntax.type_ref(ret_ty))
                .expect("string writes should not fail");
        }
        signature.push_str(&self.where_clause_opt(generics));

        signature
    }

    fn param_signature(&self, param: &ParamItem) -> String {
        match &param.ty {
            Some(ty) => format!("{}: {}", param.pat, self.syntax.type_ref(ty)),
            None => param.pat.clone(),
        }
    }

    fn item_with_fields(&self, header: String, fields: &FieldList) -> String {
        match fields {
            FieldList::Named(fields) => self.item_with_record_fields(header, fields),
            FieldList::Tuple(fields) => self.item_with_tuple_fields(header, fields),
            FieldList::Unit => header,
        }
    }

    fn item_with_record_fields(&self, header: String, fields: &[FieldItem]) -> String {
        if fields.is_empty() {
            return format!("{header} {{}}");
        }

        Self::format_block(
            header,
            fields
                .iter()
                .map(|field| self.record_field_signature(field)),
        )
    }

    fn item_with_tuple_fields(&self, header: String, fields: &[FieldItem]) -> String {
        let mut rendered = fields
            .iter()
            .take(MEMBER_PREVIEW_LIMIT)
            .map(|field| self.tuple_field_signature(field))
            .collect::<Vec<_>>();
        if fields.len() > MEMBER_PREVIEW_LIMIT {
            rendered.push("...".to_string());
        }

        format!("{header}({});", rendered.join(", "))
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

    fn record_field_signature(&self, field: &FieldItem) -> String {
        self.field_item_signature(field).unwrap_or_else(|| {
            format!(
                "{}<missing>: {}",
                Self::visibility_prefix(&field.visibility),
                self.syntax.type_ref(&field.ty)
            )
        })
    }

    fn tuple_field_signature(&self, field: &FieldItem) -> String {
        format!(
            "{}{}",
            Self::visibility_prefix(&field.visibility),
            self.syntax.type_ref(&field.ty)
        )
    }

    fn field_item_signature(&self, field: &FieldItem) -> Option<String> {
        let label = self.syntax.field_key(field.key.as_ref()?);
        Some(format!(
            "{}{}: {}",
            Self::visibility_prefix(&field.visibility),
            label,
            self.syntax.type_ref(&field.ty)
        ))
    }

    fn generic_params(&self, generics: &GenericParams) -> String {
        let mut params = Vec::new();

        params.extend(generics.lifetimes.iter().map(|param| {
            let mut text = self.syntax.name(&param.name).to_string();
            if !param.bounds.is_empty() {
                text.push_str(": ");
                for (index, bound) in param.bounds.iter().enumerate() {
                    if index > 0 {
                        text.push_str(" + ");
                    }
                    write!(text, "{}", self.syntax.name(bound))
                        .expect("string writes should not fail");
                }
            }
            text
        }));
        params.extend(generics.type_or_consts.iter().map(|param| match param {
            TypeOrConstParamData::Type(param) => {
                let mut text = self.syntax.identifier(&param.name).to_string();
                if !param.bounds.is_empty() {
                    text.push_str(": ");
                    write!(text, "{}", self.syntax.type_bounds(&param.bounds))
                        .expect("string writes should not fail");
                }
                if let Some(default) = &param.default {
                    text.push_str(" = ");
                    write!(text, "{}", self.syntax.type_ref(default))
                        .expect("string writes should not fail");
                }
                text
            }
            TypeOrConstParamData::Const(param) => {
                let mut text = format!("const {}", self.syntax.identifier(&param.name));
                if let Some(ty) = &param.ty {
                    text.push_str(": ");
                    write!(text, "{}", self.syntax.type_ref(ty))
                        .expect("string writes should not fail");
                }
                if let Some(default) = &param.default {
                    text.push_str(" = ");
                    text.push_str(default);
                }
                text
            }
        }));

        if params.is_empty() {
            String::new()
        } else {
            format!("<{}>", params.join(", "))
        }
    }

    fn generic_params_opt(&self, generics: Option<&GenericParams>) -> String {
        generics
            .map(|generics| self.generic_params(generics))
            .unwrap_or_default()
    }

    fn where_clause(&self, generics: &GenericParams) -> String {
        if generics.where_predicates.is_empty() {
            return String::new();
        }

        format!(
            " where {}",
            generics
                .where_predicates
                .iter()
                .map(|predicate| self.where_predicate(predicate))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    fn where_clause_opt(&self, generics: Option<&GenericParams>) -> String {
        generics
            .map(|generics| self.where_clause(generics))
            .unwrap_or_default()
    }

    fn where_predicate(&self, predicate: &WherePredicate) -> String {
        match predicate {
            WherePredicate::Type { ty, bounds } => {
                if bounds.is_empty() {
                    self.syntax.type_ref(ty).to_string()
                } else {
                    format!(
                        "{}: {}",
                        self.syntax.type_ref(ty),
                        self.syntax.type_bounds(bounds)
                    )
                }
            }
            WherePredicate::Lifetime { lifetime, bounds } => {
                let mut text = format!("{}: ", self.syntax.name(lifetime));
                for (index, bound) in bounds.iter().enumerate() {
                    if index > 0 {
                        text.push_str(" + ");
                    }
                    write!(text, "{}", self.syntax.name(bound))
                        .expect("string writes should not fail");
                }
                text
            }
            WherePredicate::Unsupported(text) => format!("<unsupported:{text}>"),
        }
    }
}
