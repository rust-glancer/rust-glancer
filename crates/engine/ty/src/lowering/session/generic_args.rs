//! Fill semantic generic arguments in declaration order, including inherited and defaulted values.

use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, GenericParamRef};
use rg_item_tree::{GenericArg as ItemGenericArg, TypeRef};
use rg_semantic_ir::{GenericParamSource, Generics, ItemStoreSource};
use rg_text::Name;
use rustc_type_ir::{self as ir, inherent::IntoKind};

use super::{ImplTraitMode, TypeLoweringAnchor, TypeLoweringSession, TypePathResolver};
use crate::{
    ConstValue,
    solver::{
        Const, GenericArgs, InferenceSubstitution as Substitution, InferenceTable, List, Region, Ty,
    },
};

impl<'s, 'lower, 'query, D, I, R> TypeLoweringSession<'s, 'lower, 'query, D, I, R>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    /// Lower written positional arguments against one definition's canonical parameter order.
    ///
    /// Parent arguments come from the active semantic substitution; the target's own arguments
    /// are consumed from syntax and omitted positions receive their normal semantic placeholder or
    /// default. Associated bindings belong to `lower_trait_ref`, not this positional list.
    ///
    /// With a table, written `_` and omitted function type arguments become live variables.
    /// Defaults are still interpreted in the declaration's scope; their parameter references
    /// can carry those variables without allocating new ones at the default's source site.
    pub fn lower_generic_args_for(
        &mut self,
        generics: &Generics<'_>,
        syntax_args: &[ItemGenericArg],
        inference: Option<&InferenceTable<'s>>,
    ) -> Result<GenericArgs<'s>, D::Error> {
        let mut parent_seed = Substitution::new();
        for param in generics.iter().take(generics.parent_len()) {
            if let Some(arg) = self.subst.get(param.param()) {
                parent_seed.insert(param.param(), arg);
            }
        }
        self.lower_generic_args(
            generics,
            syntax_args,
            &parent_seed,
            ImplTraitMode::Opaque,
            inference,
        )
    }

    pub(crate) fn lower_generic_args(
        &mut self,
        generics: &Generics<'_>,
        syntax_args: &[ItemGenericArg],
        seed: &Substitution<'s>,
        impl_trait_mode: ImplTraitMode,
        inference: Option<&InferenceTable<'s>>,
    ) -> Result<GenericArgs<'s>, D::Error> {
        let positional = syntax_args
            .iter()
            .filter(|arg| {
                !matches!(
                    arg,
                    ItemGenericArg::AssocType { .. } | ItemGenericArg::Unsupported(_)
                )
            })
            .collect::<Vec<_>>();
        let mut syntax_index = 0;
        let mut args = Vec::with_capacity(generics.len());
        let mut resolved = seed.clone();

        for (param_index, param) in generics.iter().enumerate() {
            if let Some(arg) = seed.get(param.param()) {
                args.push(arg);
                resolved.insert(param.param(), arg);
                continue;
            }
            if param_index < generics.parent_len() {
                let arg = self.cx.unknown_arg(param.param());
                resolved.insert(param.param(), arg);
                args.push(arg);
                continue;
            }

            let syntax = positional.get(syntax_index).copied();
            let arg = match (param.param(), syntax) {
                (GenericParamRef::Lifetime(_), Some(ItemGenericArg::Lifetime(name))) => {
                    syntax_index += 1;
                    self.lower_lifetime(name)?.into()
                }

                // Rust permits omitted lifetime args without shifting following type/const args.
                (GenericParamRef::Lifetime(_), _) => Region(ir::ReErased).into(),
                (GenericParamRef::Type(_), Some(ItemGenericArg::Type(ty))) => {
                    syntax_index += 1;
                    self.lower_type_ref_with_mode(ty, impl_trait_mode, inference)?
                        .into()
                }
                (GenericParamRef::Type(_), Some(ItemGenericArg::FnTraitArgs { params, .. })) => {
                    syntax_index += 1;
                    self.cx
                        .tuple(
                            params
                                .iter()
                                .map(|ty| {
                                    self.lower_type_ref_with_mode(ty, impl_trait_mode, inference)
                                })
                                .collect::<Result<Vec<_>, _>>()?,
                        )
                        .into()
                }
                (GenericParamRef::Const(_), Some(ItemGenericArg::Const(value))) => {
                    syntax_index += 1;
                    self.lower_const(Some(value.as_str()))?.into()
                }
                (GenericParamRef::Const(_), Some(ItemGenericArg::Type(ty)))
                    if ty.type_param_name().is_some() && !ty.has_generic_args() =>
                {
                    // In `array::IntoIter<T, N>`, bare `N` is syntactically ambiguous and the
                    // parser represents it as a type argument. The declaration says this slot is a
                    // const parameter, which resolves the ambiguity just as rustc does after
                    // parsing the generic argument list.
                    let name = ty
                        .type_param_name()
                        .expect("guard requires a plain single-segment path");
                    syntax_index += 1;
                    self.lower_const(Some(name.as_str()))?.into()
                }

                // Function calls infer omitted type parameters from their arguments and result.
                // Declaration/type lowering still uses defaults and ordinary unknown placeholders.
                (GenericParamRef::Type(_), _)
                    if matches!(generics.owner(), GenericDefRef::Function(_))
                        && inference.is_some() =>
                {
                    inference
                        .expect("call inference table")
                        .new_type_var()
                        .into()
                }
                (GenericParamRef::Type(_), _)
                    if matches!(
                        param.source(),
                        GenericParamSource::Type(source) if source.default.is_some()
                    ) =>
                {
                    let GenericParamSource::Type(source) = param.source() else {
                        unreachable!("guard accepts only source type parameters")
                    };
                    self.lower_default_type(
                        generics.owner(),
                        source
                            .default
                            .as_ref()
                            .expect("guard requires a type default"),
                        &resolved,
                    )?
                    .into()
                }
                (GenericParamRef::Const(_), _)
                    if matches!(
                        param.source(),
                        GenericParamSource::Const(source) if source.default.is_some()
                    ) =>
                {
                    let GenericParamSource::Const(source) = param.source() else {
                        unreachable!("guard accepts only source const parameters")
                    };
                    self.lower_default_const(
                        generics.owner(),
                        source
                            .default
                            .as_ref()
                            .expect("guard requires a const default")
                            .as_str(),
                        &resolved,
                    )?
                    .into()
                }
                (param, _) => self.cx.unknown_arg(param),
            };
            resolved.insert(param.param(), arg);
            args.push(arg);
        }

        Ok(List::new(self.cx, &args))
    }

    fn lower_default_type(
        &mut self,
        owner: GenericDefRef,
        ty: &TypeRef,
        subst: &Substitution<'s>,
    ) -> Result<Ty<'s>, D::Error> {
        let Some(context) = self
            .item_paths
            .items()
            .type_path_context_for_generic_def(owner)?
        else {
            return Ok(self.cx.unknown());
        };
        let previous_owner = self.owner;
        let previous_anchor = self.anchor;
        let previous_subst = std::mem::replace(&mut self.subst, subst.clone());
        self.owner = owner;
        self.anchor = TypeLoweringAnchor::Context(context);
        let result = self.lower_type_ref(ty);
        self.owner = previous_owner;
        self.anchor = previous_anchor;
        self.subst = previous_subst;
        result
    }

    fn lower_default_const(
        &mut self,
        owner: GenericDefRef,
        text: &str,
        subst: &Substitution<'s>,
    ) -> Result<Const<'s>, D::Error> {
        let Some(context) = self
            .item_paths
            .items()
            .type_path_context_for_generic_def(owner)?
        else {
            return Ok(self.cx.lower_const(ConstValue::Unknown, &[]));
        };
        let previous_owner = self.owner;
        let previous_anchor = self.anchor;
        let previous_subst = std::mem::replace(&mut self.subst, subst.clone());
        self.owner = owner;
        self.anchor = TypeLoweringAnchor::Context(context);
        let result = self.lower_const(Some(text));
        self.owner = previous_owner;
        self.anchor = previous_anchor;
        self.subst = previous_subst;
        result
    }

    pub(crate) fn lower_lifetime(&self, name: &Name) -> Result<Region<'s>, D::Error> {
        if name.as_str() == "'static" {
            return Ok(Region(ir::ReStatic));
        }
        let Some(param @ GenericParamRef::Lifetime(_)) = self.param_by_name(name.as_str())? else {
            return Ok(Region(ir::ReErased));
        };
        if let Some(arg) = self.subst.get(param)
            && let ir::GenericArgKind::Lifetime(region) = arg.kind()
        {
            return Ok(region);
        }
        let params = self.cx.generics(param.owner().into()).params;
        Ok(self
            .cx
            .param(param, params)
            .map(|param| Region(ir::ReEarlyParam(param)))
            .unwrap_or(Region(ir::ReErased)))
    }

    pub(crate) fn lower_const(&self, text: Option<&str>) -> Result<Const<'s>, D::Error> {
        let Some(text) = text else {
            return Ok(self.cx.lower_const(ConstValue::Unknown, &[]));
        };
        if let Some(param @ GenericParamRef::Const(_)) = self.param_by_name(text)? {
            if let Some(arg) = self.subst.get(param)
                && let ir::GenericArgKind::Const(value) = arg.kind()
            {
                return Ok(value);
            }
            let params = self.cx.generics(param.owner().into()).params;
            if let Some(param) = self.cx.param(param, params) {
                return Ok(Const::new(self.cx, ir::ConstKind::Param(param)));
            }
        }
        // Const evaluation is intentionally limited to the same scalar syntax supported by
        // saved types. Name resolution above preserves const parameters without evaluating them.
        Ok(self.cx.lower_const(ConstValue::from_syntax(text), &[]))
    }
}
