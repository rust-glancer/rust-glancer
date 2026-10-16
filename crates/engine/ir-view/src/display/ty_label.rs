//! Type text for presentation and for code we insert into the user's source.
//!
//! Presentation, such as inlay hints and hover, favors short, recognizable names. An `Iterator`
//! label is easier to read than its full module path.
//!
//! Refactorings and completions need names that resolve in the code we insert. For example, the
//! "Implement missing trait members" action may insert a `dyn protocol::Object` parameter into
//! a module that has not imported `Object`. Trait paths stay qualified for this use. Nominal types
//! still use short names, so their qualification or imports remain a separate limitation.
//!
//! For code we insert into an impl, the caller supplies that impl's trait application. It lets
//! `<Worker as Service<u8>>::Output` become `Self::Output` inside `impl Service<u8> for Worker`.
//! Unrelated projections remain unknown instead of being shortened to the wrong trait.

use rg_ir_model::{GenericDefRef, GenericParamRef, ItemOwner, TraitDefRef, TypeAliasRef};
use rg_semantic_ir::{GenericParamSource, GenericsQuery, ItemStoreQuery};
use rg_text::RustEdition;
use rg_ty::{
    AdtTy, AliasTy, ConstValue, GenericArg, Lifetime, OpaqueTy, ProjectionTy, Substitution,
    TraitApplication, TraitRefLowering, Ty, signature::SemanticSignatureQuery,
};

use crate::{
    IndexedViewDb, display::syntax::SyntaxRenderer, item::path::PathView, ty::IndexedType,
};

/// Renders `Ty` for readable UI labels or member signatures we insert into the user's impl.
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

    /// Render an indexed type, returning `None` when inference could not determine it.
    pub fn render(&self, ty: &IndexedType) -> anyhow::Result<Option<String>> {
        self.render_ty(ty.raw())
    }

    /// Render a compact type label for presentation, including inferred types in hover.
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
        match ty {
            Ty::Unit => Ok(Some("()".to_string())),
            Ty::Never => Ok(Some("!".to_string())),
            Ty::Primitive(primitive) => Ok(Some(primitive.label().to_string())),
            Ty::Tuple(fields) => {
                let fields = fields
                    .iter()
                    .map(|ty| {
                        self.render_ty_with_trait_impl(ty, application)
                            .map(|ty| ty.unwrap_or_else(|| "_".to_string()))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let suffix = if fields.len() == 1 { "," } else { "" };
                Ok(Some(format!("({}{suffix})", fields.join(", "))))
            }
            Ty::Array { inner, len } => Ok(Some(format!(
                "[{}; {}]",
                self.render_ty_with_trait_impl(inner, application)?
                    .unwrap_or_else(|| "_".to_string()),
                len
            ))),
            Ty::Slice(inner) => Ok(Some(format!(
                "[{}]",
                self.render_ty_with_trait_impl(inner, application)?
                    .unwrap_or_else(|| "_".to_string())
            ))),
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
                let mut inner = self
                    .render_ty_with_trait_impl(inner, application)?
                    .unwrap_or_else(|| "_".to_string());
                if parenthesize {
                    inner = format!("({inner})");
                }
                let lifetime = match lifetime {
                    rg_ty::Lifetime::Erased => String::new(),
                    lifetime => format!("{lifetime} "),
                };
                let qualifier = if matches!(mutability, rg_ir_model::Mutability::Mutable) {
                    "mut "
                } else {
                    ""
                };
                Ok(Some(format!("&{lifetime}{qualifier}{inner}")))
            }
            Ty::RawPointer { mutability, inner } => {
                // Keep extra bounds inside the pointer: `*const (dyn Service + Send)`.
                let parenthesize = matches!(
                    inner.as_ref(),
                    Ty::Dyn(object)
                        if object.lifetime != Lifetime::Erased || !object.auto_traits.is_empty()
                );
                Ok(self
                    .render_ty_with_trait_impl(inner, application)?
                    .map(|mut inner| {
                        if parenthesize {
                            inner = format!("({inner})");
                        }
                        let qualifier = if matches!(mutability, rg_ir_model::Mutability::Mutable) {
                            "mut"
                        } else {
                            "const"
                        };
                        format!("*{qualifier} {inner}")
                    }))
            }
            Ty::FnPointer { params, ret } | Ty::Closure(rg_ty::ClosureTy { params, ret, .. }) => {
                let params = params
                    .iter()
                    .map(|ty| {
                        self.render_ty_with_trait_impl(ty, application)
                            .map(|ty| ty.unwrap_or_else(|| "_".to_string()))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let ret = self
                    .render_ty_with_trait_impl(ret, application)?
                    .unwrap_or_else(|| "_".to_string());
                let params = params.join(", ");
                // Closure signatures do not record which of `Fn`, `FnMut`, or `FnOnce` applies.
                // Use `|User| -> User` so the label shows known types without guessing a bound.
                Ok(Some(if matches!(ty, Ty::Closure(_)) {
                    format!("|{params}| -> {ret}")
                } else {
                    format!("fn({params}) -> {ret}")
                }))
            }
            Ty::FnDef(function) => Ok(PathView::new(self.db, self.syntax.edition())
                .function_path(function.def)?
                .map(|path| format!("{{fn {path}}}"))),
            Ty::Adt(ty) => self.render_nominal(ty, application),
            Ty::Dyn(object) => {
                let mut bound = self.render_trait_bound(
                    object.def,
                    &object.args,
                    object
                        .associated_types
                        .iter()
                        .map(|binding| (binding.associated_ty, &binding.ty)),
                    application,
                )?;
                for auto_trait in &object.auto_traits {
                    let name = self
                        .render_trait_name(*auto_trait, application)?
                        .unwrap_or_else(|| "_".to_string());
                    bound.push_str(" + ");
                    bound.push_str(&name);
                }
                let lifetime = match object.lifetime {
                    Lifetime::Erased => String::new(),
                    lifetime => format!(" + {lifetime}"),
                };
                Ok(Some(format!("dyn {bound}{lifetime}")))
            }
            Ty::Param(param) => self.render_type_param(*param, application),
            Ty::Alias(AliasTy::Opaque(opaque)) => self.render_opaque(opaque, application),
            Ty::Alias(AliasTy::Projection(projection)) => match application {
                Some(application) => self.render_trait_impl_projection(projection, application),
                None => Ok(None),
            },
            Ty::Unknown => Ok(None),
        }
    }

    fn render_type_param(
        &self,
        param: rg_ir_model::TypeParamRef,
        application: Option<&TraitApplication>,
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
                let bounds = SemanticSignatureQuery::with_resolver(self.db, self.db, self.db)
                    .function_type_param_bounds(param)?
                    .iter()
                    .map(|bound| self.render_opaque_bound(bound, application))
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
    fn render_nominal(
        &self,
        ty: &AdtTy,
        application: Option<&TraitApplication>,
    ) -> anyhow::Result<Option<String>> {
        let Some(name) = ItemStoreQuery::new(self.db).type_def_name(ty.def)? else {
            return Ok(None);
        };

        Ok(Some(format!(
            "{}{}",
            self.syntax.identifier(name),
            self.render_generic_args(
                self.non_default_generic_args(ty.def.into(), &ty.args)?,
                application
            )?
        )))
    }

    /// Render only projections owned by the exact trait application being implemented.
    ///
    /// A projection from another trait, or from the same trait under different arguments, is not
    /// safe to shorten to `Self::Assoc`; returning `None` leaves a `_` placeholder in the signature
    /// we insert instead of giving the associated type the wrong meaning.
    fn render_trait_impl_projection(
        &self,
        projection: &ProjectionTy,
        application: &TraitApplication,
    ) -> anyhow::Result<Option<String>> {
        if projection.args != application.args {
            return Ok(None);
        }
        let Some(data) = ItemStoreQuery::new(self.db).type_alias_data(projection.associated_ty)?
        else {
            return Ok(None);
        };
        let ItemOwner::Trait(trait_id) = data.owner else {
            return Ok(None);
        };
        if (TraitDefRef {
            origin: projection.associated_ty.origin,
            id: trait_id,
        }) != application.def
        {
            return Ok(None);
        }
        Ok(Some(format!(
            "Self::{}",
            self.syntax.identifier(&data.name)
        )))
    }

    fn render_opaque(
        &self,
        opaque: &OpaqueTy,
        application: Option<&TraitApplication>,
    ) -> anyhow::Result<Option<String>> {
        let bounds = SemanticSignatureQuery::with_resolver(self.db, self.db, self.db)
            .opaque_bounds(opaque)?
            .unwrap_or_default()
            .iter()
            .map(|bound| self.render_opaque_bound(bound, application))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Some(if bounds.is_empty() {
            "impl _".to_string()
        } else {
            format!("impl {}", bounds.join(" + "))
        }))
    }

    /// Render one of the trait bounds in an `impl Trait` type.
    ///
    /// `bound.application` is the bound being displayed. The separate `application` describes
    /// the impl receiving the member we insert and must also reach types nested inside this bound.
    fn render_opaque_bound(
        &self,
        bound: &TraitRefLowering,
        application: Option<&TraitApplication>,
    ) -> anyhow::Result<String> {
        self.render_trait_bound(
            bound.application.def,
            &bound.application.args[1..],
            bound
                .associated_types
                .iter()
                .map(|binding| (binding.projection.associated_ty, &binding.ty)),
            application,
        )
    }

    /// Choose how to spell the trait named by `def`.
    ///
    /// `application` describes the impl receiving code from our completion or code action.
    /// When rendering a `dyn Object` parameter in `impl Service<u8> for Worker`, `def` identifies
    /// `Object` and `application` describes `Service<u8>`.
    ///
    /// Inlay hints and hover pass `None` and use the short name for readability. When we insert
    /// member code, callers pass `Some` and keep the full path so `Object` resolves even if the
    /// user has not imported it in the impl's module.
    fn render_trait_name(
        &self,
        def: TraitDefRef,
        application: Option<&TraitApplication>,
    ) -> anyhow::Result<Option<String>> {
        if application.is_some() {
            return PathView::new(self.db, self.syntax.edition()).trait_path(def);
        }
        Ok(ItemStoreQuery::new(self.db)
            .trait_data(def)?
            .map(|data| self.syntax.identifier(&data.name).to_string()))
    }

    /// Render `Service<Request, Output = Reply>` for either an `impl` or a `dyn` type.
    /// `args` contains `[Request]`; associated bindings are supplied separately.
    fn render_trait_bound<'ty>(
        &self,
        def: TraitDefRef,
        args: &[GenericArg],
        bindings: impl Iterator<Item = (TypeAliasRef, &'ty Ty)> + Clone,
        application: Option<&TraitApplication>,
    ) -> anyhow::Result<String> {
        if let Some(callable) =
            self.render_callable_bound(def, args, bindings.clone(), application)?
        {
            return Ok(callable);
        }

        let trait_name = self
            .render_trait_name(def, application)?
            .unwrap_or_else(|| "<trait>".to_string());
        let mut args = self
            .non_default_generic_args(def.into(), args)?
            .iter()
            .map(|arg| self.render_generic_arg(arg, application))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let items = ItemStoreQuery::new(self.db);
        for (associated_ty, ty) in bindings {
            let name = items
                .type_alias_data(associated_ty)?
                .map(|data| self.syntax.identifier(&data.name).to_string())
                .unwrap_or_else(|| "_".to_string());
            let ty = self
                .render_ty_with_trait_impl(ty, application)?
                .unwrap_or_else(|| "_".to_string());
            args.push(format!("{name} = {ty}"));
        }
        Ok(if args.is_empty() {
            trait_name
        } else {
            format!("{trait_name}<{}>", args.join(", "))
        })
    }

    /// Keep callable bounds in the familiar `Fn(u8) -> u16` form.
    fn render_callable_bound<'ty>(
        &self,
        def: TraitDefRef,
        args: &[GenericArg],
        bindings: impl Iterator<Item = (TypeAliasRef, &'ty Ty)>,
        application: Option<&TraitApplication>,
    ) -> anyhow::Result<Option<String>> {
        let items = ItemStoreQuery::new(self.db);
        let Some(trait_data) = items.trait_data(def)? else {
            return Ok(None);
        };
        if !matches!(trait_data.name.as_str(), "Fn" | "FnMut" | "FnOnce") {
            return Ok(None);
        }

        let mut positional = args.iter();
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
                self.render_ty_with_trait_impl(param, application)
                    .map(|ty| ty.unwrap_or_else(|| "_".to_string()))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut output = None;
        for (associated_ty, ty) in bindings {
            if items
                .type_alias_data(associated_ty)?
                .is_some_and(|data| data.name.as_str() == "Output")
            {
                output = Some(ty);
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
                self.render_ty_with_trait_impl(output, application)?
                    .unwrap_or_else(|| "_".to_string())
            )
        };

        Ok(Some(format!(
            "{}({}){output}",
            self.syntax.identifier(&trait_data.name),
            params.join(", ")
        )))
    }

    /// Render generic arguments including surrounding angle brackets.
    fn render_generic_args(
        &self,
        args: &[GenericArg],
        application: Option<&TraitApplication>,
    ) -> anyhow::Result<String> {
        if args.is_empty() {
            return Ok(String::new());
        }

        let mut rendered = Vec::new();
        for arg in args {
            rendered.push(self.render_generic_arg(arg, application)?);
        }

        Ok(format!("<{}>", rendered.join(", ")))
    }

    /// Leave out trailing arguments that equal their declared defaults.
    ///
    /// For `struct Pair<T = u8, U = u16>`, `Pair<u8, u16>` can be shown as `Pair`, but
    /// `Pair<u8, Custom>` needs both arguments to keep `Custom` in the second position.
    fn non_default_generic_args<'ty>(
        &self,
        owner: GenericDefRef,
        args: &'ty [GenericArg],
    ) -> anyhow::Result<&'ty [GenericArg]> {
        if args.is_empty() {
            return Ok(args);
        }
        let generics = GenericsQuery::new(self.db).generics(owner)?;
        // `trait Service<T>` has parameters `[Self, T]`, but `dyn Service<User>` supplies
        // only `[User]` here. Skip `Self` so the parameter and argument positions line up.
        let params = generics
            .iter()
            .filter(|param| !matches!(param.source(), GenericParamSource::TraitSelf))
            .collect::<Vec<_>>();
        if params.len() != args.len() {
            return Ok(args);
        }
        // A default like `U = T` needs the actual `T` before comparison. For `Pair<User, User>`,
        // substitute `T = User` so the default for `U` becomes `User` too.
        let mut substitution = Substitution::new();
        for (param, arg) in params.iter().zip(args) {
            substitution.push(param.param(), arg.clone());
        }
        let signatures = SemanticSignatureQuery::with_resolver(self.db, self.db, self.db);
        let mut keep = args.len();
        // Work backwards: once one argument must stay, all earlier slots must stay too.
        // Compare resolved values so an alias and the type it names can count as equal.
        for (param, arg) in params.iter().zip(args).rev() {
            let Some(default) = signatures.generic_param_default(param.param())? else {
                break;
            };
            let default = substitution.apply_arg(&default);
            // Two unknown values are not evidence that an argument matches its default.
            if default.has_unknown()
                || matches!(default, GenericArg::Const(ConstValue::Unknown))
                || &default != arg
            {
                break;
            }
            keep -= 1;
        }
        Ok(&args[..keep])
    }

    /// Render one generic argument.
    fn render_generic_arg(
        &self,
        arg: &GenericArg,
        application: Option<&TraitApplication>,
    ) -> anyhow::Result<String> {
        match arg {
            GenericArg::Type(ty) => Ok(self
                .render_ty_with_trait_impl(ty, application)?
                .unwrap_or_else(|| "_".to_string())),
            GenericArg::Lifetime(lifetime) => Ok(lifetime.to_string()),
            GenericArg::Const(value) => Ok(value.to_string()),
        }
    }
}
