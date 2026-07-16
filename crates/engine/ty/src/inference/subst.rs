//! Inference-aware substitutions keyed by semantic generic-parameter identity.
//!
//! A source name is useful only while lowering a declaration. Once the declaration is semantic,
//! inference binds `GenericParamRef` values directly so a method-level `T` cannot overwrite an
//! impl or trait parameter that happens to use the same spelling.

use rg_ir_model::{ConstParamRef, GenericParamRef, LifetimeParamRef, TypeParamRef};
use rg_semantic_ir::{GenericParamSource, Generics};

use super::{
    UnknownTypeInstantiationBuilder,
    table::{InferenceConflict, InferenceTable},
};
use crate::{ConstValue, GenericArg, GenericArgs, Lifetime, Substitution, Ty};

/// Generic-parameter bindings that may still contain inference variables.
///
/// This wrapper keeps trial-table unification separate from the ordinary immutable semantic
/// substitution API. Type evidence is unified through the table, while const and lifetime
/// evidence is retained in the same owner-scoped substitution so selected declarations never
/// leak their own parameters into instantiated clauses or associated values.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InferenceSubstitution(Substitution);

impl InferenceSubstitution {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_substitution(subst: Substitution) -> Self {
        Self(subst)
    }

    /// Add `param = ty`; repeated evidence must unify in the supplied trial table.
    pub fn try_push_type(
        &mut self,
        table: &mut InferenceTable,
        param: TypeParamRef,
        ty: Ty,
    ) -> Result<(), InferenceConflict> {
        if let Some(existing) = self.type_param(param).cloned() {
            // `Unknown` is the placeholder for an omitted owner argument, not established type
            // evidence. Once a call argument supplies a real type, replace that placeholder so
            // inherent associated functions can infer an impl parameter from their parameters.
            if matches!(existing, Ty::Unknown) {
                self.0
                    .push(GenericParamRef::Type(param), GenericArg::Type(Box::new(ty)));
                return Ok(());
            }

            // Receiver substitutions can carry holes below a known nominal shape, for example
            // `Self = Adapter<unknown>` before the live receiver becomes `Adapter<Closure#n>`.
            // Turn only those nested holes into trial variables, retain the known structure, and
            // let ordinary unification absorb the new evidence.
            if existing.has_unknown() {
                let existing = UnknownTypeInstantiationBuilder::new(table).ty_from_ty(&existing);
                table.try_unify(&existing, &ty)?;
                self.0.push(
                    GenericParamRef::Type(param),
                    GenericArg::Type(Box::new(existing)),
                );
                return Ok(());
            }
            return table.try_unify(&existing, &ty);
        }

        self.0
            .push(GenericParamRef::Type(param), GenericArg::Type(Box::new(ty)));
        Ok(())
    }

    pub fn push_type(&mut self, table: &mut InferenceTable, param: TypeParamRef, ty: Ty) {
        let _ = self.try_push_type(table, param, ty);
    }

    pub fn type_param(&self, param: TypeParamRef) -> Option<&Ty> {
        self.0.type_param(param)
    }

    pub(crate) fn lifetime_param(&self, param: LifetimeParamRef) -> Option<Lifetime> {
        match self.0.get(GenericParamRef::Lifetime(param)) {
            Some(GenericArg::Lifetime(lifetime)) => Some(*lifetime),
            Some(GenericArg::Type(_) | GenericArg::Const(_)) | None => None,
        }
    }

    pub(crate) fn const_param(&self, param: ConstParamRef) -> Option<ConstValue> {
        match self.0.get(GenericParamRef::Const(param)) {
            Some(GenericArg::Const(value)) => Some(*value),
            Some(GenericArg::Type(_) | GenericArg::Lifetime(_)) | None => None,
        }
    }

    pub fn as_substitution(&self) -> &Substitution {
        &self.0
    }

    pub fn into_substitution(self) -> Substitution {
        self.0
    }

    /// Convert live keyed bindings into durable positional generic arguments.
    ///
    /// `params` supplies the declaration's canonical full order, including inherited parameters.
    /// Missing bindings become kind-correct unknown arguments, and every type position is resolved
    /// through the inference table so no inference variable crosses the persistence boundary.
    pub fn finalize_args(
        &self,
        table: &InferenceTable,
        params: impl IntoIterator<Item = GenericParamRef>,
    ) -> GenericArgs {
        let args = self.0.args_for_params(params);
        table.finalize_generic_args(&args)
    }

    /// Give the function's own type parameters fresh variables, shadowing only by identity.
    pub fn shadow_type_params(&mut self, table: &mut InferenceTable, generics: &Generics<'_>) {
        for param in generics.iter_self() {
            let GenericParamRef::Type(param_ref) = param.param() else {
                continue;
            };
            if matches!(param.source(), GenericParamSource::TraitSelf) {
                continue;
            }
            self.0.push(
                GenericParamRef::Type(param_ref),
                GenericArg::Type(Box::new(table.new_type_var())),
            );
        }
    }

    /// Bind every parameter occurrence in a semantic pattern to matching evidence.
    ///
    /// Shape compatibility remains the caller's policy. This routine records only evidence that
    /// is structurally unambiguous and leaves unsupported/mismatched branches untouched.
    pub fn bind_ty(&mut self, table: &mut InferenceTable, pattern: &Ty, evidence: &Ty) {
        if let Ty::Param(param) = pattern {
            self.push_type(table, *param, evidence.clone());
            return;
        }

        match (pattern, evidence) {
            (Ty::Tuple(pattern), Ty::Tuple(evidence)) if pattern.len() == evidence.len() => {
                for (pattern, evidence) in pattern.iter().zip(evidence) {
                    self.bind_ty(table, pattern, evidence);
                }
            }
            (
                Ty::Array {
                    inner: pattern,
                    len: pattern_len,
                },
                Ty::Array {
                    inner: evidence,
                    len: evidence_len,
                },
            ) => {
                self.bind_const(*pattern_len, *evidence_len);
                self.bind_ty(table, pattern, evidence);
            }
            (Ty::Slice(pattern), Ty::Slice(evidence)) => {
                self.bind_ty(table, pattern, evidence);
            }
            (
                Ty::Reference {
                    lifetime: pattern_lifetime,
                    inner: pattern,
                    ..
                },
                Ty::Reference {
                    lifetime: evidence_lifetime,
                    inner: evidence,
                    ..
                },
            ) => {
                self.bind_lifetime(*pattern_lifetime, *evidence_lifetime);
                self.bind_ty(table, pattern, evidence);
            }
            (
                Ty::RawPointer { inner: pattern, .. },
                Ty::RawPointer {
                    inner: evidence, ..
                },
            ) => self.bind_ty(table, pattern, evidence),
            (
                Ty::FnPointer {
                    params: pattern_params,
                    ret: pattern_ret,
                },
                Ty::FnPointer {
                    params: evidence_params,
                    ret: evidence_ret,
                },
            ) if pattern_params.len() == evidence_params.len() => {
                for (pattern, evidence) in pattern_params.iter().zip(evidence_params) {
                    self.bind_ty(table, pattern, evidence);
                }
                self.bind_ty(table, pattern_ret, evidence_ret);
            }
            (Ty::Adt(pattern), Ty::Adt(evidence)) if pattern.def == evidence.def => {
                self.bind_args(table, &pattern.args, &evidence.args);
            }
            (Ty::FnDef(pattern), Ty::FnDef(evidence)) if pattern.def == evidence.def => {
                self.bind_args(table, &pattern.args, &evidence.args);
            }
            (Ty::Alias(pattern), Ty::Alias(evidence)) => {
                self.bind_args(table, pattern.args(), evidence.args());
            }
            _ => {}
        }
    }

    pub fn bind_args(
        &mut self,
        table: &mut InferenceTable,
        patterns: &[GenericArg],
        evidence: &[GenericArg],
    ) {
        if patterns.len() != evidence.len() {
            return;
        }
        for (pattern, evidence) in patterns.iter().zip(evidence) {
            match (pattern, evidence) {
                (GenericArg::Type(pattern), GenericArg::Type(evidence)) => {
                    self.bind_ty(table, pattern, evidence);
                }
                (GenericArg::Lifetime(pattern), GenericArg::Lifetime(evidence)) => {
                    self.bind_lifetime(*pattern, *evidence);
                }
                (GenericArg::Const(pattern), GenericArg::Const(evidence)) => {
                    self.bind_const(*pattern, *evidence);
                }
                _ => {}
            }
        }
    }

    /// Retain lifetime evidence without pretending to solve regions.
    ///
    /// `Erased` is a missing piece of evidence, while two distinct named/concrete lifetimes are a
    /// relationship this inference table cannot express. In the latter case the binding itself is
    /// erased so applying the substitution cannot make an order-dependent lifetime claim.
    pub(crate) fn bind_lifetime(&mut self, pattern: Lifetime, evidence: Lifetime) {
        let Lifetime::Param(param) = pattern else {
            return;
        };
        let lifetime = match self.lifetime_param(param) {
            None => evidence,
            Some(existing) if existing == evidence => existing,
            Some(Lifetime::Erased) => evidence,
            Some(existing) if evidence == Lifetime::Erased => existing,
            Some(_) => Lifetime::Erased,
        };
        self.0.push(
            GenericParamRef::Lifetime(param),
            GenericArg::Lifetime(lifetime),
        );
    }

    /// Retain const evidence, collapsing incompatible evidence to `Unknown`.
    ///
    /// A compatibility matcher can reject conflicting literals before recording them. An
    /// evidence-only caller instead gets the strongest order-independent value that can safely be
    /// substituted later.
    pub(crate) fn bind_const(&mut self, pattern: ConstValue, evidence: ConstValue) {
        let ConstValue::Param(param) = pattern else {
            return;
        };
        let value = match self.const_param(param) {
            None => evidence,
            Some(existing) if existing == evidence => existing,
            Some(ConstValue::Unknown) => evidence,
            Some(existing) if evidence == ConstValue::Unknown => existing,
            Some(_) => ConstValue::Unknown,
        };
        self.0
            .push(GenericParamRef::Const(param), GenericArg::Const(value));
    }
}
