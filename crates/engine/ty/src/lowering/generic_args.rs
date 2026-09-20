//! Fill semantic generic arguments in declaration order, including inherited and defaulted values.

use super::{ImplTraitMode, TypeLoweringAnchor, TypeLoweringSession, TypePathResolver};
use crate::inference::InferenceTable;
use crate::{ConstValue, GenericArg, GenericArgs, Lifetime, Substitution, Ty};
use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, GenericParamRef};
use rg_item_tree::{GenericArg as ItemGenericArg, TypeRef};
use rg_semantic_ir::{GenericParamSource, ItemStoreSource};

impl<'lower, 'query, D, I, R> TypeLoweringSession<'lower, 'query, D, I, R>
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
    /// With an inference table, written type placeholders `_` and omitted function types get live
    /// variables instead. For `make::<Vec<_>>()`, the inner slot can then learn from the call's use.
    pub fn lower_generic_args_for(
        &mut self,
        generics: &rg_semantic_ir::Generics<'_>,
        syntax_args: &[ItemGenericArg],
        inference: Option<&mut InferenceTable>,
    ) -> Result<GenericArgs, D::Error> {
        let mut parent_seed = Substitution::new();
        for param in generics.iter().take(generics.parent_len()) {
            if let Some(arg) = self.subst.get(param.param()) {
                parent_seed.push(param.param(), arg.clone());
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
        generics: &rg_semantic_ir::Generics<'_>,
        syntax_args: &[ItemGenericArg],
        seed: &Substitution,
        impl_trait_mode: ImplTraitMode,
        mut inference: Option<&mut InferenceTable>,
    ) -> Result<GenericArgs, D::Error> {
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
                args.push(arg.clone());
                resolved.push(param.param(), arg.clone());
                continue;
            }
            if param_index < generics.parent_len() {
                let arg = Substitution::unknown_arg(param.param());
                resolved.push(param.param(), arg.clone());
                args.push(arg);
                continue;
            }

            let syntax = positional.get(syntax_index).copied();
            let arg = match (param.param(), syntax) {
                (GenericParamRef::Lifetime(_), Some(ItemGenericArg::Lifetime(name))) => {
                    syntax_index += 1;
                    GenericArg::Lifetime(self.lower_lifetime(name)?)
                }
                // Rust permits omitted lifetime args without shifting following type/const args.
                (GenericParamRef::Lifetime(_), _) => GenericArg::Lifetime(Lifetime::Erased),
                (GenericParamRef::Type(_), Some(ItemGenericArg::Type(ty))) => {
                    syntax_index += 1;
                    GenericArg::Type(Box::new(self.lower_type_ref_with_mode(
                        ty,
                        impl_trait_mode,
                        inference.as_deref_mut(),
                    )?))
                }
                (GenericParamRef::Type(_), Some(ItemGenericArg::FnTraitArgs { params, .. })) => {
                    syntax_index += 1;
                    GenericArg::Type(Box::new(Ty::tuple(
                        params
                            .iter()
                            .map(|ty| {
                                self.lower_type_ref_with_mode(
                                    ty,
                                    impl_trait_mode,
                                    inference.as_deref_mut(),
                                )
                            })
                            .collect::<Result<_, _>>()?,
                    )))
                }
                (GenericParamRef::Const(_), Some(ItemGenericArg::Const(value))) => {
                    syntax_index += 1;
                    GenericArg::Const(self.lower_const(Some(value.as_str()))?)
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
                    GenericArg::Const(self.lower_const(Some(name.as_str()))?)
                }
                // Function calls infer omitted type parameters from their arguments and result.
                // Declaration/type lowering still uses defaults and ordinary unknown placeholders.
                (GenericParamRef::Type(_), _)
                    if matches!(generics.owner(), GenericDefRef::Function(_))
                        && inference.is_some() =>
                {
                    GenericArg::Type(Box::new(
                        inference
                            .as_deref_mut()
                            .expect("call inference table")
                            .new_type_var(),
                    ))
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
                    GenericArg::Type(Box::new(
                        self.lower_default_type(
                            generics.owner(),
                            source
                                .default
                                .as_ref()
                                .expect("guard requires a type default"),
                            &resolved,
                        )?,
                    ))
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
                    GenericArg::Const(
                        self.lower_default_const(
                            generics.owner(),
                            source
                                .default
                                .as_ref()
                                .expect("guard requires a const default")
                                .as_str(),
                            &resolved,
                        )?,
                    )
                }
                (param, _) => Substitution::unknown_arg(param),
            };
            resolved.push(param.param(), arg.clone());
            args.push(arg);
        }

        Ok(args.into())
    }

    fn lower_default_type(
        &mut self,
        owner: GenericDefRef,
        ty: &TypeRef,
        subst: &Substitution,
    ) -> Result<Ty, D::Error> {
        let Some(context) = self
            .query
            .item_paths
            .items()
            .type_path_context_for_generic_def(owner)?
        else {
            return Ok(Ty::Unknown);
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
        subst: &Substitution,
    ) -> Result<ConstValue, D::Error> {
        let Some(context) = self
            .query
            .item_paths
            .items()
            .type_path_context_for_generic_def(owner)?
        else {
            return Ok(ConstValue::Unknown);
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

    pub(crate) fn lower_lifetime(&self, name: &rg_text::Name) -> Result<Lifetime, D::Error> {
        if name.as_str() == "'static" {
            return Ok(Lifetime::Static);
        }
        Ok(match self.param_by_name(name.as_str())? {
            Some(GenericParamRef::Lifetime(param)) => {
                match self.subst.get(GenericParamRef::Lifetime(param)) {
                    Some(GenericArg::Lifetime(lifetime)) => *lifetime,
                    Some(GenericArg::Type(_)) | Some(GenericArg::Const(_)) | None => {
                        Lifetime::Param(param)
                    }
                }
            }
            _ => Lifetime::Erased,
        })
    }

    pub(crate) fn lower_const(&self, text: Option<&str>) -> Result<ConstValue, D::Error> {
        let Some(text) = text else {
            return Ok(ConstValue::Unknown);
        };
        Ok(match self.param_by_name(text)? {
            Some(GenericParamRef::Const(param)) => {
                match self.subst.get(GenericParamRef::Const(param)) {
                    Some(GenericArg::Const(value)) => *value,
                    Some(GenericArg::Type(_)) | Some(GenericArg::Lifetime(_)) | None => {
                        ConstValue::Param(param)
                    }
                }
            }
            _ => ConstValue::from_syntax(text),
        })
    }
}
