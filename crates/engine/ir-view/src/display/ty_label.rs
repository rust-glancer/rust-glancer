//! Compact type labels for UI surfaces.
//!
//! This renderer intentionally favors short, recognizable names over fully-qualified debug output.
//! The analysis layer already returns stable IDs; inlay hints and future hovers need labels that
//! are useful while reading code.

use rg_ir_model::GenericParamRef;
use rg_semantic_ir::{GenericParamSource, GenericsQuery, ItemStoreQuery};
use rg_text::RustEdition;
use rg_ty::{AdtTy, AliasTy, GenericArg, OpaqueTy, SemanticSignatureQuery, TraitRefLowering, Ty};

use crate::{IndexedViewDb, display::syntax::SyntaxRenderer, item::path::PathView};

/// Renders compact user-facing labels for `Ty`.
pub struct TypeRenderer<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
    syntax: SyntaxRenderer,
}

impl<'a, 'db> TypeRenderer<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>, edition: RustEdition) -> Self {
        Self {
            db,
            syntax: SyntaxRenderer::new(edition),
        }
    }

    /// Render a type, returning `None` for unknown types.
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
                len
            ))),
            Ty::Slice(inner) => Ok(Some(format!(
                "[{}]",
                self.render(inner)?.unwrap_or_else(|| "_".to_string())
            ))),
            Ty::Reference {
                lifetime,
                mutability,
                inner,
            } => Ok(self.render(inner)?.map(|inner| {
                let lifetime = match lifetime {
                    rg_ty::Lifetime::Erased => String::new(),
                    lifetime => format!("{lifetime} "),
                };
                let qualifier = if matches!(mutability, rg_ir_model::Mutability::Mutable) {
                    "mut "
                } else {
                    ""
                };
                format!("&{lifetime}{qualifier}{inner}")
            })),
            Ty::RawPointer { mutability, inner } => Ok(self.render(inner)?.map(|inner| {
                let qualifier = if matches!(mutability, rg_ir_model::Mutability::Mutable) {
                    "mut"
                } else {
                    "const"
                };
                format!("*{qualifier} {inner}")
            })),
            Ty::FnPointer { params, ret } => {
                let params = params
                    .iter()
                    .map(|ty| {
                        self.render(ty)
                            .map(|ty| ty.unwrap_or_else(|| "_".to_string()))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let ret = self.render(ret)?.unwrap_or_else(|| "_".to_string());
                Ok(Some(format!("fn({}) -> {ret}", params.join(", "))))
            }
            Ty::Closure(id) => Ok(Some(format!("{{closure#{id}}}"))),
            Ty::FnDef(function) => Ok(PathView::new(self.db, self.syntax.edition())
                .function_path(function.def)?
                .map(|path| format!("{{fn {path}}}"))),
            Ty::Adt(ty) => self.render_nominal(ty),
            Ty::Param(param) => self.render_type_param(*param),
            Ty::Alias(AliasTy::Opaque(opaque)) => self.render_opaque(opaque),
            Ty::Alias(AliasTy::Projection(_)) => Ok(None),
            // UI surfaces should only see finalized types. If a transient solver variable leaks
            // here, render it like unknown instead of exposing an internal slot identity.
            Ty::InferVar { .. } | Ty::Unknown => Ok(None),
        }
    }

    fn render_type_param(
        &self,
        param: rg_ir_model::TypeParamRef,
    ) -> anyhow::Result<Option<String>> {
        let generics = GenericsQuery::new(self.db).generics(param.owner)?;
        let Some(data) = generics
            .iter()
            .find(|data| data.param() == GenericParamRef::Type(param))
        else {
            return Ok(None);
        };
        match data.source() {
            GenericParamSource::TraitSelf => Ok(Some("Self".to_string())),
            GenericParamSource::Type(source) => {
                Ok(Some(self.syntax.identifier(&source.name).to_string()))
            }
            GenericParamSource::ArgumentImplTrait(_) => {
                let bounds = SemanticSignatureQuery::new(self.db, self.db)
                    .function_type_param_bounds(param)?
                    .iter()
                    .map(|bound| self.render_opaque_bound(bound))
                    .collect::<anyhow::Result<Vec<_>>>()?;
                Ok(Some(if bounds.is_empty() {
                    "impl _".to_string()
                } else {
                    format!("impl {}", bounds.join(" + "))
                }))
            }
            GenericParamSource::Lifetime(_) | GenericParamSource::Const(_) => Ok(None),
        }
    }

    /// Render a nominal type by declared name and generic arguments.
    fn render_nominal(&self, ty: &AdtTy) -> anyhow::Result<Option<String>> {
        let Some(name) = ItemStoreQuery::new(self.db).type_def_name(ty.def)? else {
            return Ok(None);
        };

        Ok(Some(format!(
            "{}{}",
            self.syntax.identifier(name),
            self.render_generic_args(&ty.args)?
        )))
    }

    fn render_opaque(&self, opaque: &OpaqueTy) -> anyhow::Result<Option<String>> {
        let bounds = SemanticSignatureQuery::new(self.db, self.db)
            .opaque_bounds(opaque)?
            .unwrap_or_default()
            .iter()
            .map(|bound| self.render_opaque_bound(bound))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Some(if bounds.is_empty() {
            "impl _".to_string()
        } else {
            format!("impl {}", bounds.join(" + "))
        }))
    }

    /// Render one declaration predicate attached to an opaque type.
    fn render_opaque_bound(&self, bound: &TraitRefLowering) -> anyhow::Result<String> {
        if let Some(callable) = self.render_callable_bound(bound)? {
            return Ok(callable);
        }

        let trait_path = PathView::new(self.db, self.syntax.edition())
            .trait_path(bound.application.def)?
            .unwrap_or_else(|| "<trait>".to_string());
        let mut args = bound
            .application
            .args
            .iter()
            .skip(1)
            .map(|arg| self.render_generic_arg(arg))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let items = ItemStoreQuery::new(self.db);
        for binding in &bound.associated_types {
            let name = items
                .type_alias_data(binding.associated_ty)?
                .map(|data| self.syntax.identifier(&data.name).to_string())
                .unwrap_or_else(|| "_".to_string());
            let ty = self.render(&binding.ty)?.unwrap_or_else(|| "_".to_string());
            args.push(format!("{name} = {ty}"));
        }
        Ok(if args.is_empty() {
            trait_path
        } else {
            format!("{trait_path}<{}>", args.join(", "))
        })
    }

    /// Preserve Rust's parenthesized presentation for the language callable traits.
    fn render_callable_bound(&self, bound: &TraitRefLowering) -> anyhow::Result<Option<String>> {
        let items = ItemStoreQuery::new(self.db);
        let Some(trait_data) = items.trait_data(bound.application.def)? else {
            return Ok(None);
        };
        if !matches!(trait_data.name.as_str(), "Fn" | "FnMut" | "FnOnce") {
            return Ok(None);
        }

        let mut positional = bound.application.args.iter().skip(1);
        let Some(GenericArg::Type(input)) = positional.next() else {
            return Ok(None);
        };
        if positional.next().is_some() {
            return Ok(None);
        }
        let Ty::Tuple(params) = input.as_ref() else {
            return Ok(None);
        };
        let params = params
            .iter()
            .map(|param| {
                self.render(param)
                    .map(|ty| ty.unwrap_or_else(|| "_".to_string()))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut output = None;
        for binding in &bound.associated_types {
            if items
                .type_alias_data(binding.associated_ty)?
                .is_some_and(|data| data.name.as_str() == "Output")
            {
                output = Some(&binding.ty);
                break;
            }
        }
        let Some(output) = output else {
            return Ok(None);
        };
        let output = if matches!(output, Ty::Unit) {
            String::new()
        } else {
            format!(
                " -> {}",
                self.render(output)?.unwrap_or_else(|| "_".to_string())
            )
        };

        Ok(Some(format!(
            "{}({}){output}",
            self.syntax.identifier(&trait_data.name),
            params.join(", ")
        )))
    }

    /// Render generic arguments including surrounding angle brackets.
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

    /// Render one generic argument.
    fn render_generic_arg(&self, arg: &GenericArg) -> anyhow::Result<String> {
        match arg {
            GenericArg::Type(ty) => Ok(self.render(ty)?.unwrap_or_else(|| "_".to_string())),
            GenericArg::Lifetime(lifetime) => Ok(lifetime.to_string()),
            GenericArg::Const(value) => Ok(value.to_string()),
        }
    }
}
