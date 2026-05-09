//! Compact type labels for UI surfaces.
//!
//! This renderer intentionally favors short, recognizable names over fully-qualified debug output.
//! The analysis layer already returns stable IDs; inlay hints and future hovers need labels that
//! are useful while reading code.

use rg_body_ir::{BodyGenericArg, BodyLocalNominalTy, BodyNominalTy, BodyTy};
use rg_semantic_ir::TypeDefId;

use super::Analysis;

pub(super) struct TypeRenderer<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> TypeRenderer<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(super) fn render(&self, ty: &BodyTy) -> anyhow::Result<Option<String>> {
        match ty {
            BodyTy::Unit => Ok(Some("()".to_string())),
            BodyTy::Never => Ok(Some("!".to_string())),
            BodyTy::Syntax(ty) => Ok(Some(ty.to_string())),
            BodyTy::Reference(inner) => Ok(self.render(inner)?.map(|inner| format!("&{inner}"))),
            BodyTy::LocalNominal(types) => {
                let mut labels = Vec::new();
                for ty in types {
                    if let Some(label) = self.render_local_nominal(ty)? {
                        labels.push(label);
                    }
                }
                Ok(Self::render_joined(labels.into_iter()))
            }
            BodyTy::Nominal(types) | BodyTy::SelfTy(types) => {
                let mut labels = Vec::new();
                for ty in types {
                    if let Some(label) = self.render_nominal(ty)? {
                        labels.push(label);
                    }
                }
                Ok(Self::render_joined(labels.into_iter()))
            }
            BodyTy::Unknown => Ok(None),
        }
    }

    fn render_joined(labels: impl Iterator<Item = String>) -> Option<String> {
        let mut labels = labels.collect::<Vec<_>>();
        labels.sort();
        (!labels.is_empty()).then(|| labels.join(" | "))
    }

    fn render_local_nominal(&self, ty: &BodyLocalNominalTy) -> anyhow::Result<Option<String>> {
        let Some(body) = self.0.body_ir.body_data(ty.item.body)? else {
            return Ok(None);
        };
        let Some(item) = body.local_item(ty.item.item) else {
            return Ok(None);
        };
        Ok(Some(format!(
            "{}{}",
            item.name,
            self.render_generic_args(&ty.args)?
        )))
    }

    fn render_nominal(&self, ty: &BodyNominalTy) -> anyhow::Result<Option<String>> {
        let Some(target_ir) = self.0.semantic_ir.target_ir(ty.def.target)? else {
            return Ok(None);
        };
        let name = match ty.def.id {
            TypeDefId::Struct(id) => {
                let Some(data) = target_ir.items().struct_data(id) else {
                    return Ok(None);
                };
                data.name.as_str()
            }
            TypeDefId::Enum(id) => {
                let Some(data) = target_ir.items().enum_data(id) else {
                    return Ok(None);
                };
                data.name.as_str()
            }
            TypeDefId::Union(id) => {
                let Some(data) = target_ir.items().union_data(id) else {
                    return Ok(None);
                };
                data.name.as_str()
            }
        };

        Ok(Some(format!(
            "{name}{}",
            self.render_generic_args(&ty.args)?
        )))
    }

    fn render_generic_args(&self, args: &[BodyGenericArg]) -> anyhow::Result<String> {
        if args.is_empty() {
            return Ok(String::new());
        }

        let mut rendered = Vec::new();
        for arg in args {
            rendered.push(self.render_generic_arg(arg)?);
        }

        Ok(format!("<{}>", rendered.join(", ")))
    }

    fn render_generic_arg(&self, arg: &BodyGenericArg) -> anyhow::Result<String> {
        match arg {
            BodyGenericArg::Type(ty) => Ok(self.render(ty)?.unwrap_or_else(|| "_".to_string())),
            BodyGenericArg::Lifetime(lifetime) => Ok(lifetime.clone()),
            BodyGenericArg::Const(value) => Ok(value.clone()),
            BodyGenericArg::AssocType { name, ty } => match ty {
                Some(ty) => Ok(format!(
                    "{name} = {}",
                    self.render(ty)?.unwrap_or_else(|| "_".to_string())
                )),
                None => Ok(name.to_string()),
            },
            BodyGenericArg::Unsupported(text) => Ok(text.clone()),
        }
    }
}
