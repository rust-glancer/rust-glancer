//! Inference-aware type-ref substitution and projection.
//!
//! This is the `InferTy` mirror of ordinary `TypeSubst` use: bind declared type params from
//! inference evidence, then project another type ref while preserving `?T` slots.

use rg_ir_model::items::{
    GenericArg as ItemGenericArg, GenericParams, Mutability, TypePath, TypeRef,
};
use rg_text::Name;
use rg_ty::{
    GenericArg, RefMutability, Ty,
    inference::{InferGenericArg, InferNominalTy, InferTy},
};

use super::BodyInferenceCtx;

/// Substitution from declared type params to inference-aware types.
///
/// Example: matching `impl<T> Vec<T>` against receiver `Vec<?T>` binds `T = ?T`.
#[derive(Debug, Default)]
pub(crate) struct InferTypeSubst(Vec<(Name, InferTy)>);

impl InferTypeSubst {
    /// Start with no inference substitutions.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Add `T = ?T`; if `T` already exists, unify both values.
    pub(crate) fn push(&mut self, inference: &mut BodyInferenceCtx, name: Name, ty: InferTy) {
        if let Some(existing) = self.get(name.as_str()).cloned() {
            inference.constrain_infer_tys(&existing, &ty);
            return;
        }

        self.0.push((name, ty));
    }

    /// Bind type params by matching declaration syntax against inference evidence.
    ///
    /// Example: `Vec<T>` matched with `Vec<?T>` binds `T = ?T`.
    pub(crate) fn bind_type_ref(
        &mut self,
        inference: &mut BodyInferenceCtx,
        pattern: &TypeRef,
        evidence: &InferTy,
        generics: &GenericParams,
    ) {
        if let Some(name) = pattern.type_param_name()
            && generics
                .types
                .iter()
                .any(|param| param.name.as_str() == name.as_str())
        {
            self.push(inference, name, evidence.clone());
            return;
        }

        match (pattern, evidence) {
            (TypeRef::Tuple(pattern_fields), InferTy::Tuple(evidence_fields))
                if pattern_fields.len() == evidence_fields.len() =>
            {
                for (pattern_field, evidence_field) in pattern_fields.iter().zip(evidence_fields) {
                    self.bind_type_ref(inference, pattern_field, evidence_field, generics);
                }
            }
            (
                TypeRef::Array {
                    inner: pattern_inner,
                    len: pattern_len,
                },
                InferTy::Array {
                    inner: evidence_inner,
                    len: evidence_len,
                },
            ) if pattern_len == evidence_len => {
                self.bind_type_ref(inference, pattern_inner, evidence_inner, generics);
            }
            (TypeRef::Slice(pattern_inner), InferTy::Slice(evidence_inner)) => {
                self.bind_type_ref(inference, pattern_inner, evidence_inner, generics);
            }
            (
                TypeRef::Reference {
                    mutability,
                    inner: pattern_inner,
                    ..
                },
                InferTy::Reference {
                    mutability: evidence_mutability,
                    inner: evidence_inner,
                },
            ) if Self::ref_mutability(*mutability) == *evidence_mutability => {
                self.bind_type_ref(inference, pattern_inner, evidence_inner, generics);
            }
            (TypeRef::Path(path), InferTy::Nominal(evidence_ty) | InferTy::SelfTy(evidence_ty)) => {
                self.bind_type_path_args(inference, path, &evidence_ty.args, generics);
            }
            _ => {}
        }
    }

    /// Return the visible binding for `T`, honoring later shadowing.
    fn get(&self, name: &str) -> Option<&InferTy> {
        self.0
            .iter()
            .rev()
            .find_map(|(param, ty)| (param.as_str() == name).then_some(ty))
    }

    /// Bind params from path args, e.g. `Vec<T>` against `Vec<?T>`.
    fn bind_type_path_args(
        &mut self,
        inference: &mut BodyInferenceCtx,
        path: &TypePath,
        evidence_args: &[InferGenericArg],
        generics: &GenericParams,
    ) {
        let Some(segment) = path.segments.last() else {
            return;
        };
        if segment.args.len() != evidence_args.len() {
            return;
        }

        for (pattern_arg, evidence_arg) in segment.args.iter().zip(evidence_args) {
            self.bind_generic_arg(inference, pattern_arg, evidence_arg, generics);
        }
    }

    /// Bind params from one generic arg, including associated-type and Fn-trait args.
    fn bind_generic_arg(
        &mut self,
        inference: &mut BodyInferenceCtx,
        pattern: &ItemGenericArg,
        evidence: &InferGenericArg,
        generics: &GenericParams,
    ) {
        match (pattern, evidence) {
            (ItemGenericArg::Type(pattern_ty), InferGenericArg::Type(evidence_ty)) => {
                self.bind_type_ref(inference, pattern_ty, evidence_ty, generics);
            }
            (
                ItemGenericArg::FnTraitArgs {
                    params: pattern_params,
                    ret: pattern_ret,
                },
                InferGenericArg::FnTraitArgs {
                    params: evidence_params,
                    ret: evidence_ret,
                },
            ) if pattern_params.len() == evidence_params.len() => {
                for (pattern_param, evidence_param) in pattern_params.iter().zip(evidence_params) {
                    self.bind_type_ref(inference, pattern_param, evidence_param, generics);
                }
                self.bind_type_ref(inference, pattern_ret, evidence_ret, generics);
            }
            (
                ItemGenericArg::AssocType {
                    name: pattern_name,
                    ty: Some(pattern_ty),
                },
                InferGenericArg::AssocType {
                    name: evidence_name,
                    ty: Some(evidence_ty),
                },
            ) if pattern_name == evidence_name => {
                self.bind_type_ref(inference, pattern_ty, evidence_ty, generics);
            }
            _ => {}
        }
    }

    /// Convert syntax mutability into the type-layer reference mutability.
    fn ref_mutability(mutability: Mutability) -> RefMutability {
        match mutability {
            Mutability::Shared => RefMutability::Shared,
            Mutability::Mutable => RefMutability::Mutable,
        }
    }
}

/// Projects declared type refs into `InferTy` using an inference substitution.
///
/// Example: `push(value: T)` with `T = ?T` projects the param type to `?T`.
pub(crate) struct InferTypeRefProjector<'subst> {
    subst: &'subst InferTypeSubst,
}

impl<'subst> InferTypeRefProjector<'subst> {
    /// Project type refs through this substitution.
    pub(crate) fn new(subst: &'subst InferTypeSubst) -> Self {
        Self { subst }
    }

    /// Resolve a declared type ref shape while preserving substituted inference vars.
    ///
    /// Example: `Option<T>` with `T = ?T` and resolved `Option<unknown>` becomes `Option<?T>`.
    pub(crate) fn ty_from_type_ref(&self, pattern: &TypeRef, resolved_ty: &Ty) -> InferTy {
        if let Some(name) = pattern.type_param_name()
            && let Some(ty) = self.subst.get(name.as_str())
        {
            return ty.clone();
        }

        match (pattern, resolved_ty) {
            (TypeRef::Unit, Ty::Unit) => InferTy::Unit,
            (TypeRef::Never, Ty::Never) => InferTy::Never,
            (TypeRef::Tuple(pattern_fields), Ty::Tuple(resolved_fields))
                if pattern_fields.len() == resolved_fields.len() =>
            {
                InferTy::Tuple(
                    pattern_fields
                        .iter()
                        .zip(resolved_fields)
                        .map(|(pattern_field, resolved_field)| {
                            self.ty_from_type_ref(pattern_field, resolved_field)
                        })
                        .collect(),
                )
            }
            (
                TypeRef::Array {
                    inner: pattern_inner,
                    len: pattern_len,
                },
                Ty::Array {
                    inner: resolved_inner,
                    len: resolved_len,
                },
            ) if pattern_len == resolved_len => InferTy::Array {
                inner: Box::new(self.ty_from_type_ref(pattern_inner, resolved_inner)),
                len: pattern_len.clone(),
            },
            (TypeRef::Slice(pattern_inner), Ty::Slice(resolved_inner)) => InferTy::Slice(Box::new(
                self.ty_from_type_ref(pattern_inner, resolved_inner),
            )),
            (
                TypeRef::Reference {
                    mutability,
                    inner: pattern_inner,
                    ..
                },
                Ty::Reference {
                    mutability: resolved_mutability,
                    inner: resolved_inner,
                },
            ) if InferTypeSubst::ref_mutability(*mutability) == *resolved_mutability => {
                InferTy::Reference {
                    mutability: *resolved_mutability,
                    inner: Box::new(self.ty_from_type_ref(pattern_inner, resolved_inner)),
                }
            }
            (
                TypeRef::Reference {
                    mutability,
                    inner: pattern_inner,
                    ..
                },
                Ty::Unknown,
            ) => InferTy::Reference {
                mutability: InferTypeSubst::ref_mutability(*mutability),
                inner: Box::new(self.ty_from_type_ref(pattern_inner, &Ty::Unknown)),
            },
            (TypeRef::Path(path), Ty::Nominal(ty)) => self
                .nominal_ty_from_path(path, ty.def, &ty.args)
                .map(InferTy::Nominal)
                .unwrap_or_else(|| InferTy::from_ty(resolved_ty)),
            (TypeRef::Path(path), Ty::SelfTy(ty)) => self
                .nominal_ty_from_path(path, ty.def, &ty.args)
                .map(InferTy::SelfTy)
                .unwrap_or_else(|| InferTy::from_ty(resolved_ty)),
            _ => InferTy::from_ty(resolved_ty),
        }
    }

    /// Project nominal path args, e.g. `Option<T>` into `Option<?T>`.
    fn nominal_ty_from_path(
        &self,
        path: &TypePath,
        def: rg_ir_model::TypeDefRef,
        resolved_args: &[GenericArg],
    ) -> Option<InferNominalTy> {
        let segment = path.segments.last()?;
        if segment.args.len() != resolved_args.len() {
            return None;
        }

        Some(InferNominalTy {
            def,
            args: segment
                .args
                .iter()
                .zip(resolved_args)
                .map(|(pattern_arg, resolved_arg)| {
                    self.generic_arg_from_item_arg(pattern_arg, resolved_arg)
                })
                .collect(),
        })
    }

    /// Project one generic arg from declaration syntax plus its resolved fallback.
    fn generic_arg_from_item_arg(
        &self,
        pattern: &ItemGenericArg,
        resolved_arg: &GenericArg,
    ) -> InferGenericArg {
        match (pattern, resolved_arg) {
            (ItemGenericArg::Type(pattern_ty), GenericArg::Type(resolved_ty)) => {
                InferGenericArg::Type(Box::new(self.ty_from_type_ref(pattern_ty, resolved_ty)))
            }
            (
                ItemGenericArg::FnTraitArgs {
                    params: pattern_params,
                    ret: pattern_ret,
                },
                GenericArg::FnTraitArgs {
                    params: resolved_params,
                    ret: resolved_ret,
                },
            ) if pattern_params.len() == resolved_params.len() => InferGenericArg::FnTraitArgs {
                params: pattern_params
                    .iter()
                    .zip(resolved_params)
                    .map(|(pattern_param, resolved_param)| {
                        self.ty_from_type_ref(pattern_param, resolved_param)
                    })
                    .collect(),
                ret: Box::new(self.ty_from_type_ref(pattern_ret, resolved_ret)),
            },
            (
                ItemGenericArg::AssocType {
                    name: pattern_name,
                    ty: Some(pattern_ty),
                },
                GenericArg::AssocType {
                    name: resolved_name,
                    ty: Some(resolved_ty),
                },
            ) if pattern_name == resolved_name => InferGenericArg::AssocType {
                name: pattern_name.clone(),
                ty: Some(Box::new(self.ty_from_type_ref(pattern_ty, resolved_ty))),
            },
            _ => Self::generic_arg_from_resolved(resolved_arg),
        }
    }

    /// Preserve a resolved arg when declaration syntax gives no inference-specific shape.
    fn generic_arg_from_resolved(resolved_arg: &GenericArg) -> InferGenericArg {
        match resolved_arg {
            GenericArg::Type(ty) => InferGenericArg::Type(Box::new(InferTy::from_ty(ty))),
            GenericArg::Lifetime(lifetime) => InferGenericArg::Lifetime(lifetime.clone()),
            GenericArg::Const(value) => InferGenericArg::Const(value.clone()),
            GenericArg::FnTraitArgs { params, ret } => InferGenericArg::FnTraitArgs {
                params: params.iter().map(InferTy::from_ty).collect(),
                ret: Box::new(InferTy::from_ty(ret)),
            },
            GenericArg::AssocType { name, ty } => InferGenericArg::AssocType {
                name: name.clone(),
                ty: ty
                    .as_ref()
                    .map(|ty| Box::new(InferTy::from_ty(ty.as_ref()))),
            },
            GenericArg::Unsupported(text) => InferGenericArg::Unsupported(text.clone()),
        }
    }
}
