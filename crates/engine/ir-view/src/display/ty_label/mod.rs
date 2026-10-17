//! Type text for presentation and for code we insert into the user's source.
//!
//! Presentation, such as inlay hints and hover, favors short, recognizable names. An `Iterator`
//! label is easier to read than its full module path.
//!
//! Inlay hints need compact inline text and concrete type details for their tooltips. Both forms
//! share a walk: surrounding syntax such as `Option<...>` is written to both outputs. A
//! simplification splits only the replaced branch: `Repeat<u8>` stays in the concrete form,
//! while the compact form gets `impl Iterator<Item = u8>`.
//!
//! Refactorings and completions need names that resolve in the code we insert. For example, the
//! "Implement missing trait members" action may insert a `dyn protocol::Object` parameter into
//! a module that has not imported `Object`. Trait paths stay qualified for this use. Nominal types
//! still use short names, so their qualification or imports remain a separate limitation.
//!
//! For code we insert into an impl, the caller supplies that impl's trait application. It lets
//! `<Worker as Service<u8>>::Output` become `Self::Output` inside `impl Service<u8> for Worker`.
//! Unrelated projections remain unknown instead of being shortened to the wrong trait.

mod output;
mod query;

use rg_ir_model::{CrateRef, GenericParamRef, TraitDefRef, TypeAliasRef};
use rg_semantic_ir::{GenericParamSource, GenericsQuery, ItemStoreQuery};
use rg_text::RustEdition;
use rg_ty::{
    AdtTy, AliasTy, GenericArg, Lifetime, OpaqueTy, TraitApplication, TraitRefLowering, Ty,
    signature::SemanticSignatureQuery,
};

pub use self::output::TypePresentation;
use self::{output::TypeOutput, query::TypeLabelQuery};
use crate::{
    IndexedViewDb, display::syntax::SyntaxRenderer, item::path::PathView, ty::IndexedType,
};

/// Renders `Ty` for readable UI labels or member signatures we insert into the user's impl.
///
/// The recursive walk owns Rust syntax. `TypeLabelQuery` answers semantic questions, such as
/// which item type an iterator yields, and `TypeOutput` handles the concrete and compact buffers.
pub struct TypeRenderer<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
    syntax: SyntaxRenderer,
    query: TypeLabelQuery<'a, 'db>,
    // Count projected Iterator::Item expansions, not ordinary nesting such as Option<Vec<T>>.
    iterator_depth: usize,
}

impl<'a, 'db> TypeRenderer<'a, 'db> {
    const MAX_ITERATOR_DEPTH: usize = 8;

    pub fn new(db: &'a IndexedViewDb<'db>, edition: RustEdition) -> Self {
        Self {
            db,
            syntax: SyntaxRenderer::new(edition),
            query: TypeLabelQuery::new(db, edition),
            iterator_depth: 0,
        }
    }

    /// Enable iterator simplification in the compact form returned by `render_presentation`.
    /// The concrete form retains the adapter and its generic arguments.
    pub fn with_iterator_simplification(mut self, use_site: CrateRef) -> anyhow::Result<Self> {
        self.query.enable_iterator_simplification(use_site)?;
        Ok(self)
    }

    /// Render a concrete UI type, returning `None` when inference could not determine it.
    pub fn render(&self, ty: &IndexedType) -> anyhow::Result<Option<String>> {
        self.render_ty(ty.raw())
    }

    /// Produce an inline label and its concrete details together. Shared structure resolves its
    /// names, bounds, and generic defaults once before writing to both forms.
    pub fn render_presentation(
        &self,
        ty: &IndexedType,
    ) -> anyhow::Result<Option<TypePresentation>> {
        let mut presentation = TypePresentation {
            concrete: String::new(),
            compact: String::new(),
        };
        let known = self.write_ty(
            ty.raw(),
            None,
            &mut TypeOutput::presentation(&mut presentation),
        )?;
        Ok(known.then_some(presentation))
    }

    /// Render the concrete type for presentation, including inferred types in hover.
    pub(crate) fn render_ty(&self, ty: &Ty) -> anyhow::Result<Option<String>> {
        self.render_ty_with_trait_impl(ty, None)
    }

    /// Render a type for a member we insert through trait-member completion or a code action.
    ///
    /// `application` identifies the impl receiving the member. Its projections use `Self::Assoc`,
    /// and trait names keep their full paths so the inserted signature does not assume imports.
    /// `Self::Assoc` remains valid even while that associated type is one of the missing members.
    ///
    /// ```text
    /// trait Service<T> {
    ///     type Output;
    ///     fn handle(&self, value: T) -> Self::Output;
    /// }
    ///
    /// // After the caller applies `Service<u8>`'s substitution:
    /// u8                              -> u8
    /// <Worker as Service<u8>>::Output -> Self::Output
    /// ```
    pub(crate) fn render_trait_impl_ty(
        &self,
        ty: &Ty,
        application: &TraitApplication,
    ) -> anyhow::Result<Option<String>> {
        self.render_ty_with_trait_impl(ty, Some(application))
    }

    /// Use `None` for presentation. Code we insert into a user's impl carries its application.
    fn render_ty_with_trait_impl(
        &self,
        ty: &Ty,
        application: Option<&TraitApplication>,
    ) -> anyhow::Result<Option<String>> {
        let mut text = String::new();
        let known = self.write_ty(ty, application, &mut TypeOutput::concrete(&mut text))?;
        Ok(known.then_some(text))
    }

    /// Walk shared type structure once, writing only the forms the caller needs.
    ///
    /// A `false` result means there is no usable label for this type. Any partial text is removed,
    /// so callers can omit a top-level hint or use `_` for a type inside another label.
    fn write_ty(
        &self,
        ty: &Ty,
        application: Option<&TraitApplication>,
        output: &mut TypeOutput<'_>,
    ) -> anyhow::Result<bool> {
        output.attempt(|output| match ty {
            Ty::Unit => {
                output.push_str("()");
                Ok(true)
            }
            Ty::Never => {
                output.push('!');
                Ok(true)
            }
            Ty::Primitive(primitive) => {
                output.push_str(primitive.label());
                Ok(true)
            }
            Ty::Tuple(fields) => {
                output.push('(');
                for (index, field) in fields.iter().enumerate() {
                    if index > 0 {
                        output.push_str(", ");
                    }
                    self.write_ty_or_unknown(field, application, output)?;
                }
                if fields.len() == 1 {
                    output.push(',');
                }
                output.push(')');
                Ok(true)
            }
            Ty::Array { inner, len } => {
                output.push('[');
                self.write_ty_or_unknown(inner, application, output)?;
                output.write(format_args!("; {len}]"));
                Ok(true)
            }
            Ty::Slice(inner) => {
                output.push('[');
                self.write_ty_or_unknown(inner, application, output)?;
                output.push(']');
                Ok(true)
            }
            Ty::Reference {
                lifetime,
                mutability,
                inner,
            } => {
                // Keep extra bounds inside the reference: `&(dyn Service + Send + 'a)`.
                let parenthesize = matches!(
                    inner.as_ref(),
                    Ty::Dyn(object)
                        if object.lifetime != Lifetime::Erased || !object.auto_traits.is_empty()
                );
                output.push('&');
                if *lifetime != Lifetime::Erased {
                    output.write(format_args!("{lifetime} "));
                }
                if matches!(mutability, rg_ir_model::Mutability::Mutable) {
                    output.push_str("mut ");
                }
                if parenthesize {
                    output.push('(');
                }
                self.write_ty_or_unknown(inner, application, output)?;
                if parenthesize {
                    output.push(')');
                }
                Ok(true)
            }
            Ty::RawPointer { mutability, inner } => {
                // Keep extra bounds inside the pointer: `*const (dyn Service + Send)`.
                let parenthesize = matches!(
                    inner.as_ref(),
                    Ty::Dyn(object)
                        if object.lifetime != Lifetime::Erased || !object.auto_traits.is_empty()
                );
                output.push_str(if matches!(mutability, rg_ir_model::Mutability::Mutable) {
                    "*mut "
                } else {
                    "*const "
                });
                if parenthesize {
                    output.push('(');
                }
                let known = self.write_ty(inner, application, output)?;
                if parenthesize {
                    output.push(')');
                }
                Ok(known)
            }
            Ty::FnPointer { params, ret } | Ty::Closure(rg_ty::ClosureTy { params, ret, .. }) => {
                // Closure signatures do not record which of `Fn`, `FnMut`, or `FnOnce` applies.
                // Use `|User| -> User` so the label shows known types without guessing a bound.
                let closure = matches!(ty, Ty::Closure(_));
                output.push_str(if closure { "|" } else { "fn(" });
                for (index, param) in params.iter().enumerate() {
                    if index > 0 {
                        output.push_str(", ");
                    }
                    self.write_ty_or_unknown(param, application, output)?;
                }
                output.push_str(if closure { "| -> " } else { ") -> " });
                self.write_ty_or_unknown(ret, application, output)?;
                Ok(true)
            }
            Ty::FnDef(function) => {
                if let Some(path) =
                    PathView::new(self.db, self.syntax.edition()).function_path(function.def)?
                {
                    output.write(format_args!("{{fn {path}}}"));
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Ty::Adt(ty) => self.write_nominal(ty, application, output),
            Ty::Dyn(object) => {
                output.push_str("dyn ");
                self.write_trait_bound(
                    object.def,
                    &object.args,
                    object
                        .associated_types
                        .iter()
                        .map(|binding| (binding.associated_ty, &binding.ty)),
                    application,
                    output,
                )?;
                for auto_trait in &object.auto_traits {
                    let name = self
                        .query
                        .trait_name(*auto_trait, application)?
                        .unwrap_or_else(|| "_".to_string());
                    output.write(format_args!(" + {name}"));
                }
                if object.lifetime != Lifetime::Erased {
                    output.write(format_args!(" + {}", object.lifetime));
                }
                Ok(true)
            }
            Ty::Param(param) => self.write_type_param(*param, application, output),
            Ty::Alias(AliasTy::Opaque(opaque)) => self.write_opaque(opaque, application, output),
            Ty::Alias(AliasTy::Projection(projection)) => match application {
                Some(application) => {
                    let Some(name) = self
                        .query
                        .trait_impl_projection_name(projection, application)?
                    else {
                        return Ok(false);
                    };
                    output.write(format_args!("Self::{}", self.syntax.identifier(name)));
                    Ok(true)
                }
                None => Ok(false),
            },
            Ty::Unknown => Ok(false),
        })
    }

    fn write_ty_or_unknown(
        &self,
        ty: &Ty,
        application: Option<&TraitApplication>,
        output: &mut TypeOutput<'_>,
    ) -> anyhow::Result<()> {
        if !self.write_ty(ty, application, output)? {
            output.push('_');
        }
        Ok(())
    }

    fn write_type_param(
        &self,
        param: rg_ir_model::TypeParamRef,
        application: Option<&TraitApplication>,
        output: &mut TypeOutput<'_>,
    ) -> anyhow::Result<bool> {
        let generics = GenericsQuery::new(self.db).generics(param.owner)?;
        let Some(data) = generics
            .iter()
            .find(|data| data.param() == GenericParamRef::Type(param))
        else {
            return Ok(false);
        };
        match data.source() {
            GenericParamSource::TraitSelf => output.push_str("Self"),
            GenericParamSource::Type(source) => {
                output.write(self.syntax.identifier(&source.name));
            }
            GenericParamSource::ArgumentImplTrait(_) => {
                // A parameter written as `impl Service` is stored as an anonymous type parameter.
                // Show its bounds instead of an internal parameter name.
                let bounds = SemanticSignatureQuery::with_resolver(self.db, self.db, self.db)
                    .function_type_param_bounds(param)?;
                output.push_str("impl ");
                if bounds.is_empty() {
                    output.push('_');
                }
                for (index, bound) in bounds.iter().enumerate() {
                    if index > 0 {
                        output.push_str(" + ");
                    }
                    self.write_opaque_bound(bound, application, output)?;
                }
            }
            GenericParamSource::Lifetime(_) | GenericParamSource::Const(_) => return Ok(false),
        }
        Ok(true)
    }

    /// Keep named types recognizable, shortening standard iterator adapters in the compact form.
    /// The callbacks describe the original spelling and a possible replacement. Output handling
    /// decides which forms need them, so this walk does not have to manage individual buffers.
    fn write_nominal(
        &self,
        ty: &AdtTy,
        application: Option<&TraitApplication>,
        output: &mut TypeOutput<'_>,
    ) -> anyhow::Result<bool> {
        output.with_compact_replacement(
            |output| {
                let Some(name) = ItemStoreQuery::new(self.db).type_def_name(ty.def)? else {
                    return Ok(false);
                };
                output.write(self.syntax.identifier(name));
                self.write_generic_args(
                    self.query
                        .non_default_generic_args(ty.def.into(), &ty.args)?,
                    application,
                    output,
                )?;
                Ok(true)
            },
            |output| {
                if application.is_some() || self.iterator_depth >= Self::MAX_ITERATOR_DEPTH {
                    return Ok(false);
                }
                let Some(item) = self.query.iterator_item(ty)? else {
                    return Ok(false);
                };

                // Associated types can lead back to this adapter, even in valid Rust:
                // `Recursive::Item = Skip<Recursive>` makes `Skip<Recursive>::Item` itself.
                // Bound these expansions, then use the concrete adapter at the limit.
                // TODO: Share recursion protection with the other type-rendering paths.
                let nested = Self {
                    db: self.db,
                    syntax: self.syntax,
                    query: self.query.clone(),
                    iterator_depth: self.iterator_depth + 1,
                };
                output.push_str("impl Iterator<Item = ");
                let known = nested.write_ty(&item, application, output)?;
                output.push('>');
                Ok(known)
            },
        )
    }

    /// An opaque `impl Trait` has an identity for inference, but its label uses its declared bounds.
    fn write_opaque(
        &self,
        opaque: &OpaqueTy,
        application: Option<&TraitApplication>,
        output: &mut TypeOutput<'_>,
    ) -> anyhow::Result<bool> {
        let bounds = SemanticSignatureQuery::with_resolver(self.db, self.db, self.db)
            .opaque_bounds(opaque)?
            .unwrap_or_default();
        output.push_str("impl ");
        if bounds.is_empty() {
            output.push('_');
        }
        for (index, bound) in bounds.iter().enumerate() {
            if index > 0 {
                output.push_str(" + ");
            }
            self.write_opaque_bound(bound, application, output)?;
        }
        Ok(true)
    }

    /// Render one of the trait bounds in an `impl Trait` type.
    ///
    /// `bound.application` is the bound being displayed. The separate `application` describes
    /// the impl receiving the member we insert and must also reach types nested inside this bound.
    fn write_opaque_bound(
        &self,
        bound: &TraitRefLowering,
        application: Option<&TraitApplication>,
        output: &mut TypeOutput<'_>,
    ) -> anyhow::Result<()> {
        self.write_trait_bound(
            bound.application.def,
            &bound.application.args[1..],
            bound
                .associated_types
                .iter()
                .map(|binding| (binding.projection.associated_ty, &binding.ty)),
            application,
            output,
        )
    }

    /// Render `Service<Request, Output = Reply>` for either an `impl` or a `dyn` type.
    /// `args` contains `[Request]`; associated bindings are supplied separately.
    fn write_trait_bound<'ty>(
        &self,
        def: TraitDefRef,
        args: &[GenericArg],
        bindings: impl Iterator<Item = (TypeAliasRef, &'ty Ty)> + Clone,
        application: Option<&TraitApplication>,
        output: &mut TypeOutput<'_>,
    ) -> anyhow::Result<()> {
        if self.write_callable_bound(def, args, bindings.clone(), application, output)? {
            return Ok(());
        }

        let trait_name = self
            .query
            .trait_name(def, application)?
            .unwrap_or_else(|| "<trait>".to_string());
        output.push_str(&trait_name);
        let args = self.query.non_default_generic_args(def.into(), args)?;
        let mut bindings = bindings.peekable();
        if args.is_empty() && bindings.peek().is_none() {
            return Ok(());
        }
        output.push('<');
        for (index, arg) in args.iter().enumerate() {
            if index > 0 {
                output.push_str(", ");
            }
            self.write_generic_arg(arg, application, output)?;
        }
        let mut separator = !args.is_empty();
        let items = ItemStoreQuery::new(self.db);
        for (associated_ty, ty) in bindings {
            if separator {
                output.push_str(", ");
            }
            separator = true;
            let name = items
                .type_alias_data(associated_ty)?
                .map(|data| self.syntax.identifier(&data.name).to_string())
                .unwrap_or_else(|| "_".to_string());
            output.write(format_args!("{name} = "));
            self.write_ty_or_unknown(ty, application, output)?;
        }
        output.push('>');
        Ok(())
    }

    /// Keep callable bounds in the familiar `Fn(u8) -> u16` form.
    fn write_callable_bound<'ty>(
        &self,
        def: TraitDefRef,
        args: &[GenericArg],
        bindings: impl Iterator<Item = (TypeAliasRef, &'ty Ty)>,
        application: Option<&TraitApplication>,
        output: &mut TypeOutput<'_>,
    ) -> anyhow::Result<bool> {
        let Some((name, params, ret)) = self.query.callable_signature(def, args, bindings)? else {
            return Ok(false);
        };
        output.write(format_args!("{}(", self.syntax.identifier(name)));
        for (index, param) in params.iter().enumerate() {
            if index > 0 {
                output.push_str(", ");
            }
            self.write_ty_or_unknown(param, application, output)?;
        }
        output.push(')');
        if !matches!(ret, Ty::Unit) {
            output.push_str(" -> ");
            self.write_ty_or_unknown(ret, application, output)?;
        }
        Ok(true)
    }

    /// Render generic arguments including surrounding angle brackets.
    fn write_generic_args(
        &self,
        args: &[GenericArg],
        application: Option<&TraitApplication>,
        output: &mut TypeOutput<'_>,
    ) -> anyhow::Result<()> {
        if !args.is_empty() {
            output.push('<');
            for (index, arg) in args.iter().enumerate() {
                if index > 0 {
                    output.push_str(", ");
                }
                self.write_generic_arg(arg, application, output)?;
            }
            output.push('>');
        }
        Ok(())
    }

    /// Render one generic argument.
    fn write_generic_arg(
        &self,
        arg: &GenericArg,
        application: Option<&TraitApplication>,
        output: &mut TypeOutput<'_>,
    ) -> anyhow::Result<()> {
        match arg {
            GenericArg::Type(ty) => self.write_ty_or_unknown(ty, application, output)?,
            GenericArg::Lifetime(lifetime) => {
                output.write(lifetime);
            }
            GenericArg::Const(value) => {
                output.write(value);
            }
        }
        Ok(())
    }
}
