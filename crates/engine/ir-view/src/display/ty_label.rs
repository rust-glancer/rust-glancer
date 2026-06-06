//! Compact type labels for UI surfaces.
//!
//! This renderer intentionally favors short, recognizable names over fully-qualified debug output.
//! The analysis layer already returns stable IDs; inlay hints and future hovers need labels that
//! are useful while reading code.

use rg_ir_storage::ItemStoreQuery;
use rg_ty::{GenericArg, NominalTy, Ty};

use crate::IndexedViewDb;

pub struct TypeRenderer<'a, 'db>(&'a IndexedViewDb<'db>);

impl<'a, 'db> TypeRenderer<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self(db)
    }

    pub fn render(&self, ty: &Ty) -> anyhow::Result<Option<String>> {
        match ty {
            Ty::Unit => Ok(Some("()".to_string())),
            Ty::Never => Ok(Some("!".to_string())),
            Ty::Primitive(primitive) => Ok(Some(primitive.label().to_string())),
            Ty::Tuple(fields) => {
                let fields = fields
                    .iter()
                    .map(|ty| {
                        self.render(ty)
                            .map(|ty| ty.unwrap_or_else(|| "_".to_string()))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let suffix = if fields.len() == 1 { "," } else { "" };
                Ok(Some(format!("({}{suffix})", fields.join(", "))))
            }
            Ty::Array { inner, len } => Ok(Some(format!(
                "[{}; {}]",
                self.render(inner)?.unwrap_or_else(|| "_".to_string()),
                len.as_deref().unwrap_or("<unknown>")
            ))),
            Ty::Slice(inner) => Ok(Some(format!(
                "[{}]",
                self.render(inner)?.unwrap_or_else(|| "_".to_string())
            ))),
            Ty::Syntax(ty) => Ok(Some(ty.to_string())),
            Ty::Reference { mutability, inner } => Ok(self
                .render(inner)?
                .map(|inner| format!("{}{inner}", mutability.render_prefix()))),
            Ty::Nominal(types) | Ty::SelfTy(types) => {
                let mut labels = Vec::new();
                for ty in types {
                    if let Some(label) = self.render_nominal(ty)? {
                        labels.push(label);
                    }
                }
                Ok(Self::render_joined(labels.into_iter()))
            }
            Ty::Unknown => Ok(None),
        }
    }

    fn render_joined(labels: impl Iterator<Item = String>) -> Option<String> {
        let mut labels = labels.collect::<Vec<_>>();
        labels.sort();
        (!labels.is_empty()).then(|| labels.join(" | "))
    }

    fn render_nominal(&self, ty: &NominalTy) -> anyhow::Result<Option<String>> {
        let Some(name) = ItemStoreQuery::new(self.0).type_def_name(ty.def)? else {
            return Ok(None);
        };

        Ok(Some(format!(
            "{name}{}",
            self.render_generic_args(&ty.args)?
        )))
    }

    fn render_generic_args(&self, args: &[GenericArg]) -> anyhow::Result<String> {
        if args.is_empty() {
            return Ok(String::new());
        }

        let mut rendered = Vec::new();
        for arg in args {
            rendered.push(self.render_generic_arg(arg)?);
        }

        Ok(format!("<{}>", rendered.join(", ")))
    }

    fn render_generic_arg(&self, arg: &GenericArg) -> anyhow::Result<String> {
        match arg {
            GenericArg::Type(ty) => Ok(self.render(ty)?.unwrap_or_else(|| "_".to_string())),
            GenericArg::Lifetime(lifetime) => Ok(lifetime.clone()),
            GenericArg::Const(value) => Ok(value.clone()),
            GenericArg::FnTraitArgs { params, ret } => {
                let params = params
                    .iter()
                    .map(|ty| {
                        self.render(ty)
                            .map(|ty| ty.unwrap_or_else(|| "_".to_string()))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?
                    .join(", ");
                let mut text = format!("({params})");
                if !matches!(ret.as_ref(), Ty::Unit) {
                    text.push_str(" -> ");
                    text.push_str(&self.render(ret)?.unwrap_or_else(|| "_".to_string()));
                }
                Ok(text)
            }
            GenericArg::AssocType { name, ty } => match ty {
                Some(ty) => Ok(format!(
                    "{name} = {}",
                    self.render(ty)?.unwrap_or_else(|| "_".to_string())
                )),
                None => Ok(name.to_string()),
            },
            GenericArg::Unsupported(text) => Ok(text.clone()),
        }
    }
}
