use std::collections::HashSet;

use super::{
    traversal::{InferenceTyFolder, same_generic_arg_shape, same_ty_shape},
    var::{InferVarId, InferVarKind},
};
use crate::{
    AdtTy, AliasTy, Clause, ClosureTy, FnDefTy, GenericArg, GenericArgs, Lifetime, OpaqueTy,
    PrimitiveTy, ProjectionTy, TraitApplication, Ty,
};

/// Marker returned when speculative inference evidence is incompatible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InferenceConflict;

#[derive(Debug, Clone, PartialEq, Eq)]
enum InferVarValue {
    /// The variable has no useful evidence yet.
    Unsolved,
    /// The variable has one chosen shape, which may still contain other variables.
    Solved(Ty),
    /// The variable saw incompatible evidence and must finalize conservatively.
    Conflict,
}

/// Fallback used when a numeric inference slot has no semantic evidence.
#[derive(Clone, Copy)]
enum NumericFallback {
    LanguageDefault,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InferVarSlot {
    kind: InferVarKind,
    value: InferVarValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnifyResult {
    Compatible { changed: bool },
    Conflict { changed: bool },
}

impl UnifyResult {
    fn compatible() -> Self {
        Self::Compatible { changed: false }
    }

    fn changed() -> Self {
        Self::Compatible { changed: true }
    }

    fn conflict() -> Self {
        Self::Conflict { changed: false }
    }

    fn changed_conflict() -> Self {
        Self::Conflict { changed: true }
    }

    fn changed_flag(self) -> bool {
        match self {
            Self::Compatible { changed } | Self::Conflict { changed } => changed,
        }
    }

    fn is_conflict(self) -> bool {
        matches!(self, Self::Conflict { .. })
    }

    fn merge(self, other: Self) -> Self {
        let changed = self.changed_flag() || other.changed_flag();
        if self.is_conflict() || other.is_conflict() {
            Self::Conflict { changed }
        } else {
            Self::Compatible { changed }
        }
    }
}

/// Tiny constraint table for inference variables.
///
/// The table owns variable slots like:
///
/// ```text
/// ?T         ordinary type variable
/// {integer} unsuffixed integer literal
/// {float}   unsuffixed float literal
/// ```
///
/// Each slot is either unsolved, solved to a `Ty`, or marked as a conflict. Inference variables
/// live inside the same `Ty` tree as every other shape, so the resolver can keep relationships
/// alive instead of collapsing them to `<unknown>`:
///
/// ```text
/// Vec<?T>
/// (&?T, bool)
/// impl Iterator<Item = ?T>
/// ```
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InferenceTable {
    slots: Vec<InferVarSlot>,
}

impl InferenceTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_type_var(&mut self) -> Ty {
        Ty::var_for_kind(InferVarKind::Type, self.alloc_var(InferVarKind::Type))
    }

    pub fn new_integer_var(&mut self) -> Ty {
        Ty::var_for_kind(InferVarKind::Integer, self.alloc_var(InferVarKind::Integer))
    }

    pub fn new_float_var(&mut self) -> Ty {
        Ty::var_for_kind(InferVarKind::Float, self.alloc_var(InferVarKind::Float))
    }

    /// Constrains two inference-aware types to be equal when the table can do so safely.
    ///
    /// Examples:
    ///
    /// ```text
    /// ?T == User                    => ?T = User
    /// Vec<?T> == Vec<User>          => ?T = User
    /// (?A, bool) == (User, bool)    => ?A = User
    /// ```
    ///
    /// This solver is intentionally conservative:
    ///
    /// - `Unknown` does not solve variables.
    /// - Different nominal definitions conflict.
    /// - Conflicts finalize to `Ty::Unknown`.
    /// - Unsolved type vars finalize to `Ty::Unknown`.
    /// - Unsolved numeric vars finalize to the existing defaults: `i32` / `f64`.
    pub fn unify(&mut self, lhs: &Ty, rhs: &Ty) -> bool {
        self.unify_ty(lhs, rhs).changed_flag()
    }

    /// Constrains two types and reports whether the evidence stayed compatible.
    ///
    /// This is useful for speculative matching: callers can clone the table, try a candidate,
    /// and discard the clone if the candidate would create a conflict.
    pub fn try_unify(&mut self, lhs: &Ty, rhs: &Ty) -> Result<(), InferenceConflict> {
        if self.unify_ty(lhs, rhs).is_conflict() {
            Err(InferenceConflict)
        } else {
            Ok(())
        }
    }

    pub fn finalize(&self, ty: &Ty) -> Ty {
        TableFinalizer::new(self, NumericFallback::LanguageDefault).fold_ty(ty)
    }

    /// Finalize inference state that stopped before reaching a fixed point.
    ///
    /// Ordinary type variables already become `Unknown`. Numeric variables must do the same here:
    /// their `i32` / `f64` defaults are language conclusions only after all available constraints
    /// have propagated.
    pub fn finalize_without_numeric_defaults(&self, ty: &Ty) -> Ty {
        TableFinalizer::new(self, NumericFallback::Unknown).fold_ty(ty)
    }

    /// Finalize every type-bearing position in a semantic argument list.
    pub(crate) fn finalize_generic_args(&self, args: &GenericArgs) -> GenericArgs {
        let mut finalizer = TableFinalizer::new(self, NumericFallback::LanguageDefault);
        args.iter()
            .map(|arg| finalizer.fold_generic_arg(arg))
            .collect()
    }

    /// Finalize durable arguments after an incomplete inference operation.
    pub(crate) fn finalize_generic_args_without_numeric_defaults(
        &self,
        args: &GenericArgs,
    ) -> GenericArgs {
        let mut finalizer = TableFinalizer::new(self, NumericFallback::Unknown);
        args.iter()
            .map(|arg| finalizer.fold_generic_arg(arg))
            .collect()
    }

    /// Expand only the root variable, preserving nested variables as future evidence links.
    pub fn resolve_root_var(&self, ty: &Ty) -> Ty {
        self.resolve_root_ty_var(ty, &mut Vec::new())
    }

    /// Return the current canonical form of an inference type.
    /// `?A = ?B` makes `Vec<?A>` compare as `Vec<?B>`;
    /// `?B = User` then makes the same value compare as `Vec<User>`.
    pub fn canonicalize(&self, ty: &Ty) -> Ty {
        TableCanonicalizer::new(self).fold_ty(ty)
    }

    /// Merge evidence into unknown children without discarding facts already established.
    ///
    /// Body inference can observe the same structural type from several directions. For example,
    /// `(Token, unknown)` arriving after `(Token, Token)` is weaker evidence, while
    /// `(unknown, Token)` can complete `(Token, unknown)`. Canonicalizing through this table first
    /// also lets solved variables participate as their established shapes.
    pub fn merge_ty_evidence(&self, existing: &Ty, evidence: &Ty) -> Ty {
        let existing = self.canonicalize(existing);
        let evidence = self.canonicalize(evidence);
        Self::refine_ty(&existing, &evidence).0
    }

    /// Return one predicate with every solved type variable expanded from this table.
    pub fn canonicalize_clause(&self, clause: &Clause) -> Clause {
        let mut canonicalizer = TableCanonicalizer::new(self);
        match clause {
            Clause::Implemented(application) => Clause::Implemented(TraitApplication {
                def: application.def,
                args: application
                    .args
                    .iter()
                    .map(|arg| canonicalizer.fold_generic_arg(arg))
                    .collect(),
            }),
            Clause::AliasEq { alias, ty } => Clause::AliasEq {
                alias: ProjectionTy {
                    associated_ty: alias.associated_ty,
                    args: alias
                        .args
                        .iter()
                        .map(|arg| canonicalizer.fold_generic_arg(arg))
                        .collect(),
                },
                ty: canonicalizer.fold_ty(ty),
            },
        }
    }

    fn alloc_var(&mut self, kind: InferVarKind) -> InferVarId {
        let id = InferVarId::from_slot_index(self.slots.len());
        self.slots.push(InferVarSlot {
            kind,
            value: InferVarValue::Unsolved,
        });
        id
    }

    fn resolve_root_ty_var(&self, ty: &Ty, active_vars: &mut Vec<InferVarId>) -> Ty {
        match ty {
            Ty::InferVar { kind, id } => self.resolve_root_var_id(*id, *kind, active_vars),
            Ty::Unit
            | Ty::Never
            | Ty::Primitive(_)
            | Ty::Tuple(_)
            | Ty::Array { .. }
            | Ty::Slice(_)
            | Ty::Reference { .. }
            | Ty::RawPointer { .. }
            | Ty::FnPointer { .. }
            | Ty::Closure(_)
            | Ty::FnDef(_)
            | Ty::Adt(_)
            | Ty::Param(_)
            | Ty::Alias(_)
            | Ty::Unknown => ty.clone(),
        }
    }

    fn resolve_root_var_id(
        &self,
        id: InferVarId,
        kind: InferVarKind,
        active_vars: &mut Vec<InferVarId>,
    ) -> Ty {
        if active_vars.contains(&id) {
            return Ty::Unknown;
        }

        let Some(slot) = self.slots.get(id.index()) else {
            return Ty::Unknown;
        };
        if slot.kind != kind {
            return Ty::Unknown;
        }

        match &slot.value {
            InferVarValue::Unsolved => Ty::var_for_kind(kind, id),
            InferVarValue::Solved(ty) => {
                active_vars.push(id);
                let resolved = self.resolve_root_ty_var(ty, active_vars);
                active_vars.pop();
                resolved
            }
            InferVarValue::Conflict => Ty::Unknown,
        }
    }

    fn unify_ty(&mut self, lhs: &Ty, rhs: &Ty) -> UnifyResult {
        // Unknown is absence of evidence, not a fresh variable. Letting it solve inference vars
        // would make "we do not know" indistinguishable from "we proved this is unknown".
        if matches!(lhs, Ty::Unknown) || matches!(rhs, Ty::Unknown) {
            return UnifyResult::compatible();
        }

        match (lhs, rhs) {
            // Variables can appear anywhere in the tree, so dispatch to the slot table before
            // comparing the surrounding structural shape.
            (Ty::InferVar { kind, id }, _) => self.unify_var(*id, *kind, rhs),
            (_, Ty::InferVar { kind, id }) => self.unify_var(*id, *kind, lhs),
            _ if !same_ty_shape(lhs, rhs) => UnifyResult::conflict(),
            (Ty::Unit, Ty::Unit)
            | (Ty::Never, Ty::Never)
            | (Ty::Primitive(_), Ty::Primitive(_))
            | (Ty::Param(_), Ty::Param(_)) => UnifyResult::compatible(),
            (Ty::Tuple(lhs_fields), Ty::Tuple(rhs_fields)) => {
                self.unify_iter(lhs_fields.iter(), rhs_fields.iter())
            }
            (
                Ty::Array {
                    inner: lhs_inner, ..
                },
                Ty::Array {
                    inner: rhs_inner, ..
                },
            ) => self.unify_ty(lhs_inner, rhs_inner),
            (Ty::Slice(lhs_inner), Ty::Slice(rhs_inner)) => self.unify_ty(lhs_inner, rhs_inner),
            (
                Ty::Reference {
                    inner: lhs_inner, ..
                },
                Ty::Reference {
                    inner: rhs_inner, ..
                },
            ) => self.unify_ty(lhs_inner, rhs_inner),
            (
                Ty::RawPointer {
                    inner: lhs_inner, ..
                },
                Ty::RawPointer {
                    inner: rhs_inner, ..
                },
            ) => self.unify_ty(lhs_inner, rhs_inner),
            (
                Ty::FnPointer {
                    params: lhs_params,
                    ret: lhs_ret,
                },
                Ty::FnPointer {
                    params: rhs_params,
                    ret: rhs_ret,
                },
            ) => self
                .unify_iter(lhs_params.iter(), rhs_params.iter())
                .merge(self.unify_ty(lhs_ret, rhs_ret)),
            (Ty::Adt(lhs_ty), Ty::Adt(rhs_ty)) => {
                // Same-definition nominal types can pass evidence through their generic arguments.
                let mut result = UnifyResult::compatible();
                for (lhs_arg, rhs_arg) in lhs_ty.args.iter().zip(&rhs_ty.args) {
                    result = result.merge(self.unify_generic_arg(lhs_arg, rhs_arg));
                }
                result
            }
            (Ty::FnDef(lhs), Ty::FnDef(rhs)) => {
                let mut result = UnifyResult::compatible();
                for (lhs_arg, rhs_arg) in lhs.args.iter().zip(&rhs.args) {
                    result = result.merge(self.unify_generic_arg(lhs_arg, rhs_arg));
                }
                result
            }
            (Ty::Closure(lhs), Ty::Closure(rhs)) => self
                .unify_iter(lhs.params.iter(), rhs.params.iter())
                .merge(self.unify_ty(&lhs.ret, &rhs.ret)),
            (Ty::Alias(lhs), Ty::Alias(rhs)) => {
                let mut result = UnifyResult::compatible();
                for (lhs_arg, rhs_arg) in lhs.args().iter().zip(rhs.args()) {
                    result = result.merge(self.unify_generic_arg(lhs_arg, rhs_arg));
                }
                result
            }
            _ => UnifyResult::conflict(),
        }
    }

    fn unify_iter<'a>(
        &mut self,
        lhs_items: impl Iterator<Item = &'a Ty>,
        rhs_items: impl Iterator<Item = &'a Ty>,
    ) -> UnifyResult {
        // Structural unification accumulates all child constraints so one tuple/argument conflict
        // does not hide other successful variable solves in the same shape.
        let mut result = UnifyResult::compatible();
        for (lhs, rhs) in lhs_items.zip(rhs_items) {
            result = result.merge(self.unify_ty(lhs, rhs));
        }
        result
    }

    fn unify_var(&mut self, id: InferVarId, kind: InferVarKind, evidence: &Ty) -> UnifyResult {
        let Some(slot) = self.slots.get(id.index()) else {
            return UnifyResult::conflict();
        };
        if slot.kind != kind {
            return UnifyResult::conflict();
        }

        let evidence = self.resolve_root_var(evidence);

        if matches!(&evidence, Ty::Unknown) {
            return UnifyResult::compatible();
        }

        // Avoid recursive solutions such as `?T = Vec<?T>`. Solved variables nested inside the
        // evidence must be followed as well: `?A = Vec<?B>` and `?B = ?T` make `?T = ?A`
        // recursive even though `?T` is not present in the stored `Vec<?B>` syntax.
        if self.ty_contains_var_or_cycle(&evidence, id) {
            let result = if Self::shallow_infer_var(&evidence).is_some_and(|(_, var)| var == id) {
                UnifyResult::compatible()
            } else {
                // Equality aliases share one representative. Poisoning only the spelling that
                // happened to occur in recursive evidence would detach it from that class and let
                // the representative accept a later concrete solution.
                let representative = self.infer_var_representative(id, kind);
                self.mark_conflict(representative)
            };
            return result;
        }

        match self.slots[id.index()].value.clone() {
            InferVarValue::Unsolved => self.solve_unsolved_var(id, &evidence),
            InferVarValue::Solved(existing) => {
                let result = self.unify_ty(&existing, &evidence);
                if result.is_conflict() {
                    return self.mark_conflict(id).merge(result);
                }

                // A slot may first learn a weak shape like `Vec<unknown>` and later see the same
                // shape with real inference links, e.g. `Vec<?T>`. Keep the stronger child facts.
                let (refined, refined_changed) = Self::refine_ty(&existing, &evidence);
                if refined_changed {
                    self.slots[id.index()].value = InferVarValue::Solved(refined);
                    result.merge(UnifyResult::changed())
                } else {
                    result
                }
            }
            InferVarValue::Conflict => UnifyResult::conflict(),
        }
    }

    /// Follow solved slots while checking whether evidence would make a variable recursive.
    ///
    /// The active stack also rejects evidence that already contains a recursive slot graph. Such
    /// a graph should not be constructible through this table, but accepting it as new evidence
    /// would spread the invalid state and make later type traversal unsafe.
    fn ty_contains_var_or_cycle<'a>(&'a self, ty: &'a Ty, needle: InferVarId) -> bool {
        // A solved structural chain can be much longer than the source type that created each
        // individual slot. Use an explicit DFS so malformed/generated evidence cannot consume the
        // thread stack, and retain both visiting/finished sets so shared DAG nodes are not mistaken
        // for cycles.
        enum Step<'ty> {
            Visit(&'ty Ty),
            FinishVar(InferVarId),
        }

        let mut steps = vec![Step::Visit(ty)];
        let mut active_vars = HashSet::new();
        let mut finished_vars = HashSet::new();
        while let Some(step) = steps.pop() {
            let ty = match step {
                Step::Visit(ty) => ty,
                Step::FinishVar(id) => {
                    active_vars.remove(&id);
                    finished_vars.insert(id);
                    continue;
                }
            };

            match ty {
                Ty::InferVar { kind, id } => {
                    if *id == needle || active_vars.contains(id) {
                        return true;
                    }
                    if finished_vars.contains(id) {
                        continue;
                    }

                    let Some(slot) = self.slots.get(id.index()) else {
                        continue;
                    };
                    if slot.kind != *kind {
                        continue;
                    }
                    let InferVarValue::Solved(solution) = &slot.value else {
                        continue;
                    };

                    active_vars.insert(*id);
                    steps.push(Step::FinishVar(*id));
                    steps.push(Step::Visit(solution));
                }
                Ty::Tuple(fields) => {
                    steps.extend(fields.iter().map(Step::Visit));
                }
                Ty::Array { inner, .. }
                | Ty::Slice(inner)
                | Ty::Reference { inner, .. }
                | Ty::RawPointer { inner, .. } => steps.push(Step::Visit(inner)),
                Ty::FnPointer { params, ret } => {
                    steps.push(Step::Visit(ret));
                    steps.extend(params.iter().map(Step::Visit));
                }
                Ty::Adt(ty) => {
                    steps.extend(
                        ty.args
                            .iter()
                            .filter_map(GenericArg::as_ty)
                            .map(Step::Visit),
                    );
                }
                Ty::Alias(alias) => {
                    steps.extend(
                        alias
                            .args()
                            .iter()
                            .filter_map(GenericArg::as_ty)
                            .map(Step::Visit),
                    );
                }
                Ty::FnDef(function) => {
                    steps.extend(
                        function
                            .args
                            .iter()
                            .filter_map(GenericArg::as_ty)
                            .map(Step::Visit),
                    );
                }
                Ty::Closure(closure) => {
                    steps.push(Step::Visit(&closure.ret));
                    steps.extend(closure.params.iter().map(Step::Visit));
                }
                Ty::Unit | Ty::Never | Ty::Primitive(_) | Ty::Param(_) | Ty::Unknown => {}
            }
        }
        false
    }

    /// Follow variable-to-variable links to the slot that owns their shared evidence.
    fn infer_var_representative(&self, id: InferVarId, kind: InferVarKind) -> InferVarId {
        let mut current_id = id;
        let mut current_kind = kind;
        let mut visited = HashSet::new();
        while visited.insert(current_id) {
            let Some(slot) = self.slots.get(current_id.index()) else {
                break;
            };
            if slot.kind != current_kind {
                break;
            }
            let InferVarValue::Solved(Ty::InferVar {
                kind: next_kind,
                id: next_id,
            }) = &slot.value
            else {
                break;
            };
            let Some(next_slot) = self.slots.get(next_id.index()) else {
                break;
            };
            if next_slot.kind != *next_kind {
                break;
            }
            current_id = *next_id;
            current_kind = *next_kind;
        }
        current_id
    }

    fn solve_unsolved_var(&mut self, id: InferVarId, evidence: &Ty) -> UnifyResult {
        let kind = self.slots[id.index()].kind;
        // Equality between same-kind variables is symmetric, so keep the oldest slot as the
        // representative. Directional links based on call order build chains such as
        // `?0 -> ?1 -> ?2`; every later canonicalization then recursively walks that chain.
        // Stable representatives instead make later variables point back into the body-owned
        // slots that existed before them.
        if let Some((evidence_kind, evidence_id)) = Self::shallow_infer_var(evidence)
            && evidence_kind == kind
        {
            let (representative, alias) = if id.index() <= evidence_id.index() {
                (id, evidence_id)
            } else {
                (evidence_id, id)
            };
            if representative == alias {
                return UnifyResult::compatible();
            }

            debug_assert!(matches!(
                self.slots[representative.index()].value,
                InferVarValue::Unsolved
            ));
            debug_assert!(matches!(
                self.slots[alias.index()].value,
                InferVarValue::Unsolved
            ));
            self.slots[alias.index()].value =
                InferVarValue::Solved(Ty::var_for_kind(kind, representative));
            return UnifyResult::changed();
        }

        // Numeric variables may be unified with an ordinary type variable. Link through the type
        // variable so a later or already-known primitive solution is shared by both slots.
        if let Some((var_kind, var)) = Self::shallow_infer_var(evidence)
            && var_kind == InferVarKind::Type
            && self.slots[var.index()].kind == InferVarKind::Type
            && kind != InferVarKind::Type
        {
            return self.unify_var(var, InferVarKind::Type, &Ty::var_for_kind(kind, id));
        }

        if !self.var_kind_accepts(kind, evidence) {
            return self.mark_conflict(id);
        }

        self.slots[id.index()].value = InferVarValue::Solved(evidence.clone());
        UnifyResult::changed()
    }

    fn mark_conflict(&mut self, id: InferVarId) -> UnifyResult {
        let slot = &mut self.slots[id.index()];
        if matches!(slot.value, InferVarValue::Conflict) {
            return UnifyResult::conflict();
        }

        slot.value = InferVarValue::Conflict;
        UnifyResult::changed_conflict()
    }

    fn var_kind_accepts(&self, kind: InferVarKind, evidence: &Ty) -> bool {
        match kind {
            InferVarKind::Type => !matches!(evidence, Ty::Unknown),
            InferVarKind::Integer => match evidence {
                Ty::Primitive(primitive) => primitive.is_integral(),
                Ty::InferVar {
                    kind: InferVarKind::Integer,
                    ..
                } => true,
                // If evidence is `Type`, it can later resolve to e.g. `u64`.
                Ty::InferVar {
                    kind: InferVarKind::Type,
                    id,
                } => self.slots[id.index()].kind == InferVarKind::Type,
                _ => false,
            },
            InferVarKind::Float => match evidence {
                Ty::Primitive(primitive) => primitive.is_float(),
                Ty::InferVar {
                    kind: InferVarKind::Float,
                    ..
                } => true,
                // If evidence is `Type`, it can later resolve to e.g. `f64`.
                Ty::InferVar {
                    kind: InferVarKind::Type,
                    id,
                } => self.slots[id.index()].kind == InferVarKind::Type,
                _ => false,
            },
        }
    }

    fn shallow_infer_var(ty: &Ty) -> Option<(InferVarKind, InferVarId)> {
        match ty {
            Ty::InferVar { kind, id } => Some((*kind, *id)),
            Ty::Unit
            | Ty::Never
            | Ty::Primitive(_)
            | Ty::Tuple(_)
            | Ty::Array { .. }
            | Ty::Slice(_)
            | Ty::Reference { .. }
            | Ty::RawPointer { .. }
            | Ty::FnPointer { .. }
            | Ty::Closure(_)
            | Ty::FnDef(_)
            | Ty::Adt(_)
            | Ty::Param(_)
            | Ty::Alias(_)
            | Ty::Unknown => None,
        }
    }

    fn unify_generic_arg(&mut self, lhs: &GenericArg, rhs: &GenericArg) -> UnifyResult {
        match (lhs, rhs) {
            // Type generic args are direct nested type positions.
            (GenericArg::Type(lhs), GenericArg::Type(rhs)) => self.unify_ty(lhs, rhs),

            _ => {
                if lhs == rhs {
                    UnifyResult::compatible()
                } else {
                    UnifyResult::conflict()
                }
            }
        }
    }

    /// Merge later evidence into weak children of an already chosen slot shape.
    /// `Vec<unknown>` plus `Vec<?T>` becomes `Vec<?T>`.
    fn refine_ty(existing: &Ty, evidence: &Ty) -> (Ty, bool) {
        if matches!(evidence, Ty::Unknown) {
            return (existing.clone(), false);
        }
        if matches!(existing, Ty::Unknown) {
            return (evidence.clone(), true);
        }
        if !same_ty_shape(existing, evidence) {
            return (existing.clone(), false);
        }

        match (existing, evidence) {
            (Ty::Tuple(existing_fields), Ty::Tuple(evidence_fields)) => {
                let (fields, changed) =
                    Self::refine_ty_iter(existing_fields.iter(), evidence_fields.iter());
                (Ty::Tuple(fields), changed)
            }
            (
                Ty::Array {
                    inner: existing_inner,
                    len: existing_len,
                },
                Ty::Array {
                    inner: evidence_inner,
                    ..
                },
            ) => {
                let (inner, changed) = Self::refine_ty(existing_inner, evidence_inner);
                (
                    Ty::Array {
                        inner: Box::new(inner),
                        len: *existing_len,
                    },
                    changed,
                )
            }
            (Ty::Slice(existing_inner), Ty::Slice(evidence_inner)) => {
                let (inner, changed) = Self::refine_ty(existing_inner, evidence_inner);
                (Ty::Slice(Box::new(inner)), changed)
            }
            (
                Ty::Reference {
                    lifetime: existing_lifetime,
                    mutability: existing_mutability,
                    inner: existing_inner,
                },
                Ty::Reference {
                    inner: evidence_inner,
                    ..
                },
            ) => {
                let (inner, changed) = Self::refine_ty(existing_inner, evidence_inner);
                (
                    Ty::Reference {
                        lifetime: *existing_lifetime,
                        mutability: *existing_mutability,
                        inner: Box::new(inner),
                    },
                    changed,
                )
            }
            (
                Ty::RawPointer {
                    mutability: existing_mutability,
                    inner: existing_inner,
                },
                Ty::RawPointer {
                    inner: evidence_inner,
                    ..
                },
            ) => {
                let (inner, changed) = Self::refine_ty(existing_inner, evidence_inner);
                (
                    Ty::RawPointer {
                        mutability: *existing_mutability,
                        inner: Box::new(inner),
                    },
                    changed,
                )
            }
            (
                Ty::FnPointer {
                    params: existing_params,
                    ret: existing_ret,
                },
                Ty::FnPointer {
                    params: evidence_params,
                    ret: evidence_ret,
                },
            ) => {
                let (params, params_changed) =
                    Self::refine_ty_iter(existing_params.iter(), evidence_params.iter());
                let (ret, ret_changed) = Self::refine_ty(existing_ret, evidence_ret);
                (
                    Ty::FnPointer {
                        params,
                        ret: Box::new(ret),
                    },
                    params_changed || ret_changed,
                )
            }
            (Ty::Adt(existing_ty), Ty::Adt(evidence_ty)) => {
                let (args, changed) =
                    Self::refine_generic_args(&existing_ty.args, &evidence_ty.args);
                (
                    Ty::Adt(AdtTy {
                        def: existing_ty.def,
                        args: args.into(),
                    }),
                    changed,
                )
            }
            (Ty::FnDef(existing_ty), Ty::FnDef(evidence_ty)) => {
                let (args, changed) =
                    Self::refine_generic_args(&existing_ty.args, &evidence_ty.args);
                (
                    Ty::FnDef(FnDefTy {
                        def: existing_ty.def,
                        args: args.into(),
                    }),
                    changed,
                )
            }
            (Ty::Closure(existing_ty), Ty::Closure(evidence_ty)) => {
                let (params, params_changed) =
                    Self::refine_ty_iter(existing_ty.params.iter(), evidence_ty.params.iter());
                let (ret, ret_changed) = Self::refine_ty(&existing_ty.ret, &evidence_ty.ret);
                (
                    Ty::Closure(ClosureTy {
                        id: existing_ty.id,
                        params,
                        ret: Box::new(ret),
                    }),
                    params_changed || ret_changed,
                )
            }
            (Ty::Alias(existing_ty), Ty::Alias(evidence_ty)) => {
                let (args, changed) =
                    Self::refine_generic_args(existing_ty.args(), evidence_ty.args());
                let alias = match existing_ty {
                    AliasTy::Projection(existing_ty) => AliasTy::Projection(ProjectionTy {
                        associated_ty: existing_ty.associated_ty,
                        args: args.into(),
                    }),
                    AliasTy::Opaque(existing_ty) => AliasTy::Opaque(OpaqueTy {
                        opaque: existing_ty.opaque,
                        args: args.into(),
                    }),
                };
                (Ty::Alias(alias), changed)
            }
            _ => (existing.clone(), false),
        }
    }

    fn refine_ty_iter<'a>(
        existing: impl Iterator<Item = &'a Ty>,
        evidence: impl Iterator<Item = &'a Ty>,
    ) -> (Vec<Ty>, bool) {
        let mut changed = false;
        let fields = existing
            .zip(evidence)
            .map(|(existing, evidence)| {
                let (field, field_changed) = Self::refine_ty(existing, evidence);
                changed |= field_changed;
                field
            })
            .collect();
        (fields, changed)
    }

    fn refine_generic_args(
        existing: &[GenericArg],
        evidence: &[GenericArg],
    ) -> (Vec<GenericArg>, bool) {
        let mut changed = false;
        let args = existing
            .iter()
            .zip(evidence)
            .map(|(existing, evidence)| {
                let (arg, arg_changed) = Self::refine_generic_arg(existing, evidence);
                changed |= arg_changed;
                arg
            })
            .collect();
        (args, changed)
    }

    fn refine_generic_arg(existing: &GenericArg, evidence: &GenericArg) -> (GenericArg, bool) {
        if !same_generic_arg_shape(existing, evidence) {
            return (existing.clone(), false);
        }

        match (existing, evidence) {
            (GenericArg::Type(existing), GenericArg::Type(evidence)) => {
                let (ty, changed) = Self::refine_ty(existing, evidence);
                (GenericArg::Type(Box::new(ty)), changed)
            }
            _ => (existing.clone(), false),
        }
    }
}

/// Builds canonical comparison shapes from table roots.
struct TableCanonicalizer<'table> {
    table: &'table InferenceTable,
    active_vars: Vec<InferVarId>,
}

impl<'table> TableCanonicalizer<'table> {
    fn new(table: &'table InferenceTable) -> Self {
        Self {
            table,
            active_vars: Vec::new(),
        }
    }
}

impl InferenceTyFolder for TableCanonicalizer<'_> {
    fn fold_infer_var(&mut self, id: InferVarId, kind: InferVarKind) -> Ty {
        if self.active_vars.contains(&id) {
            return Ty::Unknown;
        }

        let Some(slot) = self.table.slots.get(id.index()) else {
            return Ty::Unknown;
        };
        if slot.kind != kind {
            return Ty::Unknown;
        }

        match &slot.value {
            InferVarValue::Unsolved => Ty::var_for_kind(kind, id),
            InferVarValue::Solved(ty) => {
                self.active_vars.push(id);
                let canonical = self.fold_ty(ty);
                self.active_vars.pop();
                canonical
            }
            InferVarValue::Conflict => Ty::Unknown,
        }
    }
}

/// Finalizes inference variables while the shared folder owns the surrounding type traversal.
struct TableFinalizer<'table> {
    table: &'table InferenceTable,
    active_vars: Vec<InferVarId>,
    numeric_fallback: NumericFallback,
}

impl<'table> TableFinalizer<'table> {
    fn new(table: &'table InferenceTable, numeric_fallback: NumericFallback) -> Self {
        Self {
            table,
            active_vars: Vec::new(),
            numeric_fallback,
        }
    }
}

impl InferenceTyFolder for TableFinalizer<'_> {
    fn fold_tuple(&mut self, fields: &[Ty]) -> Ty {
        Ty::tuple(fields.iter().map(|field| self.fold_ty(field)).collect())
    }

    fn fold_array(&mut self, inner: &Ty, len: &crate::ConstValue) -> Ty {
        Ty::array(self.fold_ty(inner), *len)
    }

    fn fold_slice(&mut self, inner: &Ty) -> Ty {
        Ty::slice(self.fold_ty(inner))
    }

    fn fold_reference(
        &mut self,
        lifetime: Lifetime,
        mutability: crate::Mutability,
        inner: &Ty,
    ) -> Ty {
        Ty::reference_with_lifetime(lifetime, mutability, self.fold_ty(inner))
    }

    fn fold_infer_var(&mut self, id: InferVarId, kind: InferVarKind) -> Ty {
        // A defensive cycle check keeps bad intermediate links from escaping as recursive types.
        if self.active_vars.contains(&id) {
            return Ty::Unknown;
        }

        let Some(slot) = self.table.slots.get(id.index()) else {
            return Ty::Unknown;
        };
        if slot.kind != kind {
            return Ty::Unknown;
        }

        match &slot.value {
            InferVarValue::Unsolved => match (kind, self.numeric_fallback) {
                (InferVarKind::Type, _)
                | (InferVarKind::Integer | InferVarKind::Float, NumericFallback::Unknown) => {
                    Ty::Unknown
                }
                (InferVarKind::Integer, NumericFallback::LanguageDefault) => {
                    Ty::Primitive(PrimitiveTy::DEFAULT_INT)
                }
                (InferVarKind::Float, NumericFallback::LanguageDefault) => {
                    Ty::Primitive(PrimitiveTy::DEFAULT_FLOAT)
                }
            },
            InferVarValue::Solved(ty) => {
                self.active_vars.push(id);
                let finalized = self.fold_ty(ty);
                self.active_vars.pop();

                // Numeric variables may only publish numeric primitives. If a bad link slipped
                // through, finalization drops it rather than exposing a plausible wrong type.
                match (kind, &finalized) {
                    (InferVarKind::Type, _) => finalized,
                    (InferVarKind::Integer, Ty::Primitive(primitive))
                        if primitive.is_integral() =>
                    {
                        finalized
                    }
                    (InferVarKind::Float, Ty::Primitive(primitive)) if primitive.is_float() => {
                        finalized
                    }
                    (InferVarKind::Integer | InferVarKind::Float, _) => Ty::Unknown,
                }
            }
            InferVarValue::Conflict => Ty::Unknown,
        }
    }
}
