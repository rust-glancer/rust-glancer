//! Inference-aware type-ref substitution and projection.
//!
//! This is the `InferTy` mirror of ordinary `TypeSubst` use: bind declared type params from
//! inference evidence, then project another type ref while preserving `?T` slots.

use rg_ir_model::items::{
    GenericArg as ItemGenericArg, GenericParams, Mutability, TypePath, TypeRef,
};
use rg_text::Name;

use super::{
    family::TypeRefInferenceProjector,
    model::{InferGenericArg, InferTy},
    table::{InferenceConflict, InferenceTable},
};
use crate::{RefMutability, Ty};

/// Substitution from declared type params to inference-aware types.
///
/// Example: matching `impl<T> Vec<T>` against receiver `Vec<?T>` binds `T = ?T`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InferTypeSubst(Vec<(Name, InferTy)>);

impl InferTypeSubst {
    /// Start with no inference substitutions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add `T = ?T`; if `T` already exists, unify both values.
    pub fn push(&mut self, table: &mut InferenceTable, name: Name, ty: InferTy) {
        let _ = self.try_push(table, name, ty);
    }

    /// Add `T = ?T` and report whether repeated evidence stayed compatible.
    pub fn try_push(
        &mut self,
        table: &mut InferenceTable,
        name: Name,
        ty: InferTy,
    ) -> Result<(), InferenceConflict> {
        if let Some(existing) = self.get(name.as_str()).cloned() {
            return table.try_unify(&existing, &ty);
        }

        self.0.push((name, ty));
        Ok(())
    }

    /// Return the visible inference binding for a type parameter.
    pub fn type_param(&self, name: &str) -> Option<InferTy> {
        self.get(name).cloned()
    }

    /// Let function generics hide same-named impl generics while staying inferable.
    pub fn shadow_type_params(&mut self, table: &mut InferenceTable, generics: &GenericParams) {
        for param in &generics.types {
            self.0.push((param.name.clone(), table.new_type_var()));
        }
    }

    /// Bind type params by matching declaration syntax against inference evidence.
    ///
    /// Example: `Vec<T>` matched with `Vec<?T>` binds `T = ?T`.
    pub fn bind_type_ref(
        &mut self,
        table: &mut InferenceTable,
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
            self.push(table, name, evidence.clone());
            return;
        }

        match (pattern, evidence) {
            (TypeRef::Tuple(pattern_fields), InferTy::Tuple(evidence_fields))
                if pattern_fields.len() == evidence_fields.len() =>
            {
                for (pattern_field, evidence_field) in pattern_fields.iter().zip(evidence_fields) {
                    self.bind_type_ref(table, pattern_field, evidence_field, generics);
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
                self.bind_type_ref(table, pattern_inner, evidence_inner, generics);
            }
            (TypeRef::Slice(pattern_inner), InferTy::Slice(evidence_inner)) => {
                self.bind_type_ref(table, pattern_inner, evidence_inner, generics);
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
                self.bind_type_ref(table, pattern_inner, evidence_inner, generics);
            }
            (TypeRef::Path(path), InferTy::Nominal(evidence_ty) | InferTy::SelfTy(evidence_ty)) => {
                self.bind_type_path_args(table, path, &evidence_ty.args, generics);
            }
            _ => {}
        }
    }

    /// Bind declared type params from inferred args, e.g. `Option<?T>` gives `T = ?T`.
    pub fn bind_type_params_from_infer_args(
        &mut self,
        table: &mut InferenceTable,
        generics: &GenericParams,
        args: &[InferGenericArg],
    ) {
        let type_args = args.iter().filter_map(|arg| match arg {
            InferGenericArg::Type(ty) => Some(ty.as_ref().clone()),
            InferGenericArg::Lifetime(_)
            | InferGenericArg::Const(_)
            | InferGenericArg::FnTraitArgs { .. }
            | InferGenericArg::AssocType { .. }
            | InferGenericArg::Unsupported(_) => None,
        });

        for (param, ty) in generics.types.iter().zip(type_args) {
            self.push(table, param.name.clone(), ty);
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
        table: &mut InferenceTable,
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
            self.bind_generic_arg(table, pattern_arg, evidence_arg, generics);
        }
    }

    /// Bind params from one generic arg, including associated-type and Fn-trait args.
    fn bind_generic_arg(
        &mut self,
        table: &mut InferenceTable,
        pattern: &ItemGenericArg,
        evidence: &InferGenericArg,
        generics: &GenericParams,
    ) {
        match (pattern, evidence) {
            (ItemGenericArg::Type(pattern_ty), InferGenericArg::Type(evidence_ty)) => {
                self.bind_type_ref(table, pattern_ty, evidence_ty, generics);
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
                    self.bind_type_ref(table, pattern_param, evidence_param, generics);
                }
                self.bind_type_ref(table, pattern_ret, evidence_ret, generics);
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
                self.bind_type_ref(table, pattern_ty, evidence_ty, generics);
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
pub struct InferTypeRefProjector<'subst> {
    subst: &'subst InferTypeSubst,
}

impl<'subst> InferTypeRefProjector<'subst> {
    /// Project type refs through this substitution.
    pub fn new(subst: &'subst InferTypeSubst) -> Self {
        Self { subst }
    }

    /// Resolve a declared type ref shape while preserving substituted inference vars.
    ///
    /// Example: `Option<T>` with `T = ?T` and resolved `Option<unknown>` becomes `Option<?T>`.
    pub fn ty_from_type_ref(&mut self, pattern: &TypeRef, resolved_ty: &Ty) -> InferTy {
        self.project_ty(pattern, resolved_ty)
    }

    /// Resolve a generic arg shape while preserving substituted inference vars.
    pub fn generic_arg_from_arg(
        &mut self,
        pattern: &ItemGenericArg,
        resolved_arg: &crate::GenericArg,
    ) -> InferGenericArg {
        self.project_generic_arg(pattern, resolved_arg)
    }
}

impl TypeRefInferenceProjector for InferTypeRefProjector<'_> {
    /// Substitute declared type params such as `T` with already-bound inference vars.
    fn replace_written_ty(&mut self, pattern: &TypeRef) -> Option<InferTy> {
        let name = pattern.type_param_name()?;
        self.subst.get(name.as_str()).cloned()
    }
}
