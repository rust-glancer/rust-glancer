//! Compact type labels for UI surfaces.
//!
//! This renderer intentionally favors short, recognizable names over fully-qualified debug output.
//! The analysis layer already returns stable IDs; inlay hints and future hovers need labels that
//! are useful while reading code.

use rg_ir_model::identity::DeclarationRef;
use rg_ty::{IndexedGenericArg, IndexedNominalTy, IndexedTy, IndexedTyRepr};

use crate::{IndexedViewDb, item::declaration::DeclarationView};

pub struct TypeRenderer<'a, 'db>(&'a IndexedViewDb<'db>);

impl<'a, 'db> TypeRenderer<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self(db)
    }

    pub fn render(&self, ty: &IndexedTy) -> anyhow::Result<Option<String>> {
        match ty {
            IndexedTy::Unit => Ok(Some("()".to_string())),
            IndexedTy::Never => Ok(Some("!".to_string())),
            IndexedTy::Primitive(primitive) => Ok(Some(primitive.label().to_string())),
            IndexedTy::Repr(IndexedTyRepr::Syntax(ty)) => Ok(Some(ty.to_string())),
            IndexedTy::Reference { mutability, inner } => Ok(self
                .render(inner)?
                .map(|inner| format!("{}{inner}", mutability.render_prefix()))),
            IndexedTy::Repr(IndexedTyRepr::Nominal(types) | IndexedTyRepr::SelfTy(types)) => {
                let mut labels = Vec::new();
                for ty in types {
                    if let Some(label) = self.render_nominal(ty)? {
                        labels.push(label);
                    }
                }
                Ok(Self::render_joined(labels.into_iter()))
            }
            IndexedTy::Unknown => Ok(None),
        }
    }

    fn render_joined(labels: impl Iterator<Item = String>) -> Option<String> {
        let mut labels = labels.collect::<Vec<_>>();
        labels.sort();
        (!labels.is_empty()).then(|| labels.join(" | "))
    }

    fn render_nominal(&self, ty: &IndexedNominalTy) -> anyhow::Result<Option<String>> {
        let Some(declaration) =
            DeclarationView::new(self.0).declaration(DeclarationRef::semantic(ty.def.into()))?
        else {
            return Ok(None);
        };
        let name = declaration.name();

        Ok(Some(format!(
            "{name}{}",
            self.render_generic_args(&ty.args)?
        )))
    }

    fn render_generic_args(&self, args: &[IndexedGenericArg]) -> anyhow::Result<String> {
        if args.is_empty() {
            return Ok(String::new());
        }

        let mut rendered = Vec::new();
        for arg in args {
            rendered.push(self.render_generic_arg(arg)?);
        }

        Ok(format!("<{}>", rendered.join(", ")))
    }

    fn render_generic_arg(&self, arg: &IndexedGenericArg) -> anyhow::Result<String> {
        match arg {
            IndexedGenericArg::Type(ty) => Ok(self.render(ty)?.unwrap_or_else(|| "_".to_string())),
            IndexedGenericArg::Lifetime(lifetime) => Ok(lifetime.clone()),
            IndexedGenericArg::Const(value) => Ok(value.clone()),
            IndexedGenericArg::AssocType { name, ty } => match ty {
                Some(ty) => Ok(format!(
                    "{name} = {}",
                    self.render(ty)?.unwrap_or_else(|| "_".to_string())
                )),
                None => Ok(name.to_string()),
            },
            IndexedGenericArg::Unsupported(text) => Ok(text.clone()),
        }
    }
}
