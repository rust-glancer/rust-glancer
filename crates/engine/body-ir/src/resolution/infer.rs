//! Body-local inference facts used before writing resolved `Ty` values.
//!
//! The persisted Body IR model stores ordinary `Ty` facts. This module keeps inference variables
//! in a transient table so later resolver phases can preserve relationships such as `Vec<?T>`
//! until local evidence solves `?T`.

use rg_ir_model::items::TypeRef;
use rg_ir_model::{TraitRef, TypeDefRef};
use rg_std::UniqueVec;
use rg_text::Name;
use rg_ty::{GenericArg, NominalTy, OpaqueTraitBound, PrimitiveTy, RefMutability, Ty};

pub(super) struct BodyInferenceCtx {
    table: InferenceTable,
    expr_tys: Vec<InferTy>,
    binding_tys: Vec<InferTy>,
}

impl BodyInferenceCtx {
    pub(super) fn new(expr_count: usize, binding_count: usize) -> Self {
        Self {
            table: InferenceTable::new(),
            expr_tys: vec![InferTy::Unknown; expr_count],
            binding_tys: vec![InferTy::Unknown; binding_count],
        }
    }

    pub(super) fn table(&self) -> &InferenceTable {
        &self.table
    }

    pub(super) fn table_mut(&mut self) -> &mut InferenceTable {
        &mut self.table
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct InferVarId(u32);

impl InferVarId {
    fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InferVarKind {
    /// Ordinary type variable, e.g. `?T`.
    Type,
    /// Numeric literal variable that can only settle to an integral primitive.
    Integer,
    /// Numeric literal variable that can only settle to a floating-point primitive.
    Float,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InferVarValue {
    /// The variable has no useful evidence yet.
    Unsolved,
    /// The variable has one chosen shape, which may still contain other variables.
    Solved(InferTy),
    /// The variable saw incompatible evidence and must finalize conservatively.
    Conflict,
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

/// Tiny body-local constraint table for inference variables.
///
/// The table owns variable slots like:
///
/// ```text
/// ?T         ordinary type variable
/// {integer} unsuffixed integer literal
/// {float}   unsuffixed float literal
/// ```
///
/// Each slot is either unsolved, solved to an `InferTy`, or marked as a conflict. `InferTy`
/// mirrors the `Ty` shapes we care about, but adds variables inside the tree. That means the
/// resolver can keep relationships alive instead of collapsing them to `<unknown>`:
///
/// ```text
/// Vec<?T>
/// (&?T, bool)
/// impl Iterator<Item = ?T>
/// ```
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct InferenceTable {
    slots: Vec<InferVarSlot>,
}

impl InferenceTable {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn new_type_var(&mut self) -> InferTy {
        InferTy::Var(self.alloc_var(InferVarKind::Type))
    }

    pub(super) fn new_integer_var(&mut self) -> InferTy {
        InferTy::IntegerVar(self.alloc_var(InferVarKind::Integer))
    }

    pub(super) fn new_float_var(&mut self) -> InferTy {
        InferTy::FloatVar(self.alloc_var(InferVarKind::Float))
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
    /// - Ambiguous nominal sets do not solve.
    /// - Conflicts finalize to `Ty::Unknown`.
    /// - Unsolved type vars finalize to `Ty::Unknown`.
    /// - Unsolved numeric vars finalize to the existing defaults: `i32` / `f64`.
    pub(super) fn unify(&mut self, lhs: &InferTy, rhs: &InferTy) -> bool {
        self.unify_ty(lhs, rhs).changed_flag()
    }

    pub(super) fn finalize(&self, ty: &InferTy) -> Ty {
        self.finalize_ty(ty, &mut Vec::new())
    }

    fn alloc_var(&mut self, kind: InferVarKind) -> InferVarId {
        let id = InferVarId(
            self.slots
                .len()
                .try_into()
                .expect("one body should not allocate more than u32::MAX inference variables"),
        );
        self.slots.push(InferVarSlot {
            kind,
            value: InferVarValue::Unsolved,
        });
        id
    }

    fn unify_ty(&mut self, lhs: &InferTy, rhs: &InferTy) -> UnifyResult {
        // Unknown is absence of evidence, not a fresh variable. Letting it solve inference vars
        // would make "we do not know" indistinguishable from "we proved this is unknown".
        if matches!(lhs, InferTy::Unknown) || matches!(rhs, InferTy::Unknown) {
            return UnifyResult::compatible();
        }

        match (lhs, rhs) {
            // Variables can appear anywhere in the tree, so dispatch to the slot table before
            // comparing the surrounding structural shape.
            (InferTy::Var(id) | InferTy::IntegerVar(id) | InferTy::FloatVar(id), _) => {
                self.unify_var(*id, rhs)
            }
            (_, InferTy::Var(id) | InferTy::IntegerVar(id) | InferTy::FloatVar(id)) => {
                self.unify_var(*id, lhs)
            }
            (InferTy::Unit, InferTy::Unit)
            | (InferTy::Never, InferTy::Never)
            | (InferTy::Primitive(_), InferTy::Primitive(_))
            | (InferTy::Syntax(_), InferTy::Syntax(_)) => {
                if lhs == rhs {
                    UnifyResult::compatible()
                } else {
                    UnifyResult::conflict()
                }
            }
            (InferTy::Tuple(lhs_fields), InferTy::Tuple(rhs_fields))
                if lhs_fields.len() == rhs_fields.len() =>
            {
                self.unify_iter(lhs_fields.iter(), rhs_fields.iter())
            }
            (
                InferTy::Array {
                    inner: lhs_inner,
                    len: lhs_len,
                },
                InferTy::Array {
                    inner: rhs_inner,
                    len: rhs_len,
                },
            ) if lhs_len == rhs_len => self.unify_ty(lhs_inner, rhs_inner),
            (InferTy::Slice(lhs_inner), InferTy::Slice(rhs_inner)) => {
                self.unify_ty(lhs_inner, rhs_inner)
            }
            (
                InferTy::Reference {
                    mutability: lhs_mutability,
                    inner: lhs_inner,
                },
                InferTy::Reference {
                    mutability: rhs_mutability,
                    inner: rhs_inner,
                },
            ) if lhs_mutability == rhs_mutability => self.unify_ty(lhs_inner, rhs_inner),
            (InferTy::Nominal(lhs_types), InferTy::Nominal(rhs_types))
            | (InferTy::SelfTy(lhs_types), InferTy::SelfTy(rhs_types)) => {
                self.unify_nominal_sets(lhs_types, rhs_types)
            }
            (InferTy::Opaque { bounds: lhs_bounds }, InferTy::Opaque { bounds: rhs_bounds }) => {
                self.unify_opaque_bounds(lhs_bounds, rhs_bounds)
            }
            _ => UnifyResult::conflict(),
        }
    }

    fn unify_iter<'a>(
        &mut self,
        lhs_items: impl Iterator<Item = &'a InferTy>,
        rhs_items: impl Iterator<Item = &'a InferTy>,
    ) -> UnifyResult {
        // Structural unification accumulates all child constraints so one tuple/argument conflict
        // does not hide other successful variable solves in the same shape.
        let mut result = UnifyResult::compatible();
        for (lhs, rhs) in lhs_items.zip(rhs_items) {
            result = result.merge(self.unify_ty(lhs, rhs));
        }
        result
    }

    fn unify_var(&mut self, id: InferVarId, evidence: &InferTy) -> UnifyResult {
        // Syntax placeholders are preserved facts, not solver evidence. Later phases may resolve
        // them first and feed the resolved shape back into the table.
        if matches!(evidence, InferTy::Unknown | InferTy::Syntax(_)) {
            return UnifyResult::compatible();
        }

        // Avoid recursive solutions such as `?T = Vec<?T>`. A variable equal to itself is fine,
        // but an actual cycle would make finalization order-dependent.
        if self.ty_contains_var(evidence, id) {
            return if matches!(evidence, InferTy::Var(var) | InferTy::IntegerVar(var) | InferTy::FloatVar(var) if *var == id)
            {
                UnifyResult::compatible()
            } else {
                self.mark_conflict(id)
            };
        }

        match self.slots[id.index()].value.clone() {
            InferVarValue::Unsolved => self.solve_unsolved_var(id, evidence),
            InferVarValue::Solved(existing) => {
                let result = self.unify_ty(&existing, evidence);
                if result.is_conflict() {
                    return self.mark_conflict(id).merge(result);
                }
                result
            }
            InferVarValue::Conflict => UnifyResult::conflict(),
        }
    }

    fn solve_unsolved_var(&mut self, id: InferVarId, evidence: &InferTy) -> UnifyResult {
        let kind = self.slots[id.index()].kind;
        // Numeric variables may be unified with an ordinary type variable. Link through the type
        // variable so a later or already-known primitive solution is shared by both slots.
        if let Some(var) = evidence.var_id()
            && self.slots[var.index()].kind == InferVarKind::Type
            && kind != InferVarKind::Type
        {
            return self.unify_var(var, &InferTy::var_for_kind(kind, id));
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

    fn var_kind_accepts(&self, kind: InferVarKind, evidence: &InferTy) -> bool {
        match kind {
            InferVarKind::Type => !matches!(evidence, InferTy::Unknown | InferTy::Syntax(_)),
            InferVarKind::Integer => match evidence {
                InferTy::Primitive(primitive) => primitive.is_integral(),
                InferTy::IntegerVar(_) => true,
                InferTy::Var(id) => self.slots[id.index()].kind == InferVarKind::Type,
                _ => false,
            },
            InferVarKind::Float => match evidence {
                InferTy::Primitive(primitive) => primitive.is_float(),
                InferTy::FloatVar(_) => true,
                InferTy::Var(id) => self.slots[id.index()].kind == InferVarKind::Type,
                _ => false,
            },
        }
    }

    fn unify_nominal_sets(
        &mut self,
        lhs_types: &UniqueVec<InferNominalTy>,
        rhs_types: &UniqueVec<InferNominalTy>,
    ) -> UnifyResult {
        // Nominal candidate sets can be ambiguous. Only one clear same-definition pair lets us
        // descend into generic arguments without accidentally picking a candidate.
        let ([lhs], [rhs]) = (lhs_types.as_slice(), rhs_types.as_slice()) else {
            return UnifyResult::compatible();
        };
        if lhs.def != rhs.def {
            return UnifyResult::conflict();
        }
        if lhs.args.len() != rhs.args.len() {
            return UnifyResult::conflict();
        }

        let mut result = UnifyResult::compatible();
        for (lhs_arg, rhs_arg) in lhs.args.iter().zip(&rhs.args) {
            result = result.merge(self.unify_generic_arg(lhs_arg, rhs_arg));
        }
        result
    }

    fn unify_opaque_bounds(
        &mut self,
        lhs_bounds: &UniqueVec<InferOpaqueTraitBound>,
        rhs_bounds: &UniqueVec<InferOpaqueTraitBound>,
    ) -> UnifyResult {
        // Opaque bounds follow the same rule as nominal candidates: only a single matching trait
        // bound is precise enough to use its generic arguments as evidence.
        let ([lhs], [rhs]) = (lhs_bounds.as_slice(), rhs_bounds.as_slice()) else {
            return UnifyResult::compatible();
        };
        if lhs.trait_ref != rhs.trait_ref {
            return UnifyResult::conflict();
        }
        if lhs.args.len() != rhs.args.len() {
            return UnifyResult::conflict();
        }

        let mut result = UnifyResult::compatible();
        for (lhs_arg, rhs_arg) in lhs.args.iter().zip(&rhs.args) {
            result = result.merge(self.unify_generic_arg(lhs_arg, rhs_arg));
        }
        result
    }

    fn unify_generic_arg(&mut self, lhs: &InferGenericArg, rhs: &InferGenericArg) -> UnifyResult {
        match (lhs, rhs) {
            (InferGenericArg::Type(lhs), InferGenericArg::Type(rhs)) => self.unify_ty(lhs, rhs),
            // Parenthesized `Fn*` args carry real type positions, so preserve them as ordinary
            // nested constraints instead of treating the whole argument as opaque text.
            (
                InferGenericArg::FnTraitArgs {
                    params: lhs_params,
                    ret: lhs_ret,
                },
                InferGenericArg::FnTraitArgs {
                    params: rhs_params,
                    ret: rhs_ret,
                },
            ) if lhs_params.len() == rhs_params.len() => self
                .unify_iter(lhs_params.iter(), rhs_params.iter())
                .merge(self.unify_ty(lhs_ret, rhs_ret)),
            (
                InferGenericArg::AssocType {
                    name: lhs_name,
                    ty: lhs_ty,
                },
                InferGenericArg::AssocType {
                    name: rhs_name,
                    ty: rhs_ty,
                },
            ) if lhs_name == rhs_name => match (lhs_ty, rhs_ty) {
                (Some(lhs_ty), Some(rhs_ty)) => self.unify_ty(lhs_ty, rhs_ty),
                // A missing associated type equality carries no evidence, but it also should not
                // poison the surrounding trait-bound unification.
                (None, None) => UnifyResult::compatible(),
                (Some(_), None) | (None, Some(_)) => UnifyResult::compatible(),
            },
            _ => {
                if lhs == rhs {
                    UnifyResult::compatible()
                } else {
                    UnifyResult::conflict()
                }
            }
        }
    }

    fn ty_contains_var(&self, ty: &InferTy, needle: InferVarId) -> bool {
        match ty {
            InferTy::Var(id) | InferTy::IntegerVar(id) | InferTy::FloatVar(id) => *id == needle,
            InferTy::Tuple(fields) => fields
                .iter()
                .any(|field| self.ty_contains_var(field, needle)),
            InferTy::Array { inner, .. }
            | InferTy::Slice(inner)
            | InferTy::Reference { inner, .. } => self.ty_contains_var(inner, needle),
            InferTy::Opaque { bounds } => bounds.iter().any(|bound| {
                bound
                    .args
                    .iter()
                    .any(|arg| self.generic_arg_contains_var(arg, needle))
            }),
            InferTy::Nominal(types) | InferTy::SelfTy(types) => types.iter().any(|ty| {
                ty.args
                    .iter()
                    .any(|arg| self.generic_arg_contains_var(arg, needle))
            }),
            InferTy::Unit
            | InferTy::Never
            | InferTy::Primitive(_)
            | InferTy::Syntax(_)
            | InferTy::Unknown => false,
        }
    }

    fn generic_arg_contains_var(&self, arg: &InferGenericArg, needle: InferVarId) -> bool {
        match arg {
            InferGenericArg::Type(ty) => self.ty_contains_var(ty, needle),
            InferGenericArg::FnTraitArgs { params, ret } => {
                params
                    .iter()
                    .any(|param| self.ty_contains_var(param, needle))
                    || self.ty_contains_var(ret, needle)
            }
            InferGenericArg::AssocType { ty, .. } => ty
                .as_deref()
                .is_some_and(|ty| self.ty_contains_var(ty, needle)),
            InferGenericArg::Lifetime(_)
            | InferGenericArg::Const(_)
            | InferGenericArg::Unsupported(_) => false,
        }
    }

    fn finalize_ty(&self, ty: &InferTy, active_vars: &mut Vec<InferVarId>) -> Ty {
        // Finalization is the only place inference variables become persisted `Ty` facts. Keep it
        // structural so partially solved containers retain the pieces we did learn.
        match ty {
            InferTy::Unit => Ty::Unit,
            InferTy::Never => Ty::Never,
            InferTy::Primitive(primitive) => Ty::Primitive(*primitive),
            InferTy::Tuple(fields) => Ty::tuple(
                fields
                    .iter()
                    .map(|field| self.finalize_ty(field, active_vars))
                    .collect(),
            ),
            InferTy::Array { inner, len } => {
                Ty::array(self.finalize_ty(inner, active_vars), len.clone())
            }
            InferTy::Slice(inner) => Ty::slice(self.finalize_ty(inner, active_vars)),
            InferTy::Reference { mutability, inner } => {
                Ty::reference(*mutability, self.finalize_ty(inner, active_vars))
            }
            InferTy::Opaque { bounds } => Ty::opaque(
                bounds
                    .iter()
                    .map(|bound| self.finalize_opaque_bound(bound, active_vars))
                    .collect(),
            ),
            InferTy::Syntax(ty) => Ty::syntax(ty.as_ref().clone()),
            InferTy::Nominal(types) => Ty::nominal(
                types
                    .iter()
                    .map(|ty| self.finalize_nominal_ty(ty, active_vars))
                    .collect(),
            ),
            InferTy::SelfTy(types) => Ty::self_ty(
                types
                    .iter()
                    .map(|ty| self.finalize_nominal_ty(ty, active_vars))
                    .collect(),
            ),
            InferTy::Var(id) => self.finalize_var(*id, InferVarKind::Type, active_vars),
            InferTy::IntegerVar(id) => self.finalize_var(*id, InferVarKind::Integer, active_vars),
            InferTy::FloatVar(id) => self.finalize_var(*id, InferVarKind::Float, active_vars),
            InferTy::Unknown => Ty::Unknown,
        }
    }

    fn finalize_var(
        &self,
        id: InferVarId,
        kind: InferVarKind,
        active_vars: &mut Vec<InferVarId>,
    ) -> Ty {
        // A defensive cycle check keeps bad intermediate links from escaping as recursive types.
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
            InferVarValue::Unsolved => match kind {
                InferVarKind::Type => Ty::Unknown,
                InferVarKind::Integer => Ty::Primitive(PrimitiveTy::DEFAULT_INT),
                InferVarKind::Float => Ty::Primitive(PrimitiveTy::DEFAULT_FLOAT),
            },
            InferVarValue::Solved(ty) => {
                // Numeric variables may only publish numeric primitives. If a bad link slipped
                // through, finalization drops it rather than exposing a plausible wrong type.
                active_vars.push(id);
                let finalized = self.finalize_ty(ty, active_vars);
                active_vars.pop();

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

    fn finalize_nominal_ty(
        &self,
        ty: &InferNominalTy,
        active_vars: &mut Vec<InferVarId>,
    ) -> NominalTy {
        NominalTy {
            def: ty.def,
            args: ty
                .args
                .iter()
                .map(|arg| self.finalize_generic_arg(arg, active_vars))
                .collect(),
        }
    }

    fn finalize_opaque_bound(
        &self,
        bound: &InferOpaqueTraitBound,
        active_vars: &mut Vec<InferVarId>,
    ) -> OpaqueTraitBound {
        OpaqueTraitBound {
            trait_ref: bound.trait_ref,
            args: bound
                .args
                .iter()
                .map(|arg| self.finalize_generic_arg(arg, active_vars))
                .collect(),
        }
    }

    fn finalize_generic_arg(
        &self,
        arg: &InferGenericArg,
        active_vars: &mut Vec<InferVarId>,
    ) -> GenericArg {
        match arg {
            InferGenericArg::Type(ty) => {
                GenericArg::Type(Box::new(self.finalize_ty(ty, active_vars)))
            }
            InferGenericArg::Lifetime(lifetime) => GenericArg::Lifetime(lifetime.clone()),
            InferGenericArg::Const(value) => GenericArg::Const(value.clone()),
            InferGenericArg::FnTraitArgs { params, ret } => GenericArg::FnTraitArgs {
                params: params
                    .iter()
                    .map(|param| self.finalize_ty(param, active_vars))
                    .collect(),
                ret: Box::new(self.finalize_ty(ret, active_vars)),
            },
            InferGenericArg::AssocType { name, ty } => GenericArg::AssocType {
                name: name.clone(),
                ty: ty
                    .as_deref()
                    .map(|ty| Box::new(self.finalize_ty(ty, active_vars))),
            },
            InferGenericArg::Unsupported(text) => GenericArg::Unsupported(text.clone()),
        }
    }
}

/// Inference-aware mirror of `Ty`.
///
/// This type is transient and body-local. It can carry variables inside the same shapes persisted
/// `Ty` already supports, then finalize back to `Ty` once the body pass is done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum InferTy {
    Unit,
    Never,
    Primitive(PrimitiveTy),
    Tuple(Vec<InferTy>),
    Array {
        inner: Box<InferTy>,
        len: Option<String>,
    },
    Slice(Box<InferTy>),
    Reference {
        mutability: RefMutability,
        inner: Box<InferTy>,
    },
    Opaque {
        bounds: UniqueVec<InferOpaqueTraitBound>,
    },
    Syntax(Box<TypeRef>),
    Nominal(UniqueVec<InferNominalTy>),
    SelfTy(UniqueVec<InferNominalTy>),
    Var(InferVarId),
    IntegerVar(InferVarId),
    FloatVar(InferVarId),
    Unknown,
}

impl InferTy {
    pub(super) fn from_ty(ty: &Ty) -> Self {
        match ty {
            Ty::Unit => Self::Unit,
            Ty::Never => Self::Never,
            Ty::Primitive(primitive) => Self::Primitive(*primitive),
            Ty::Tuple(fields) => Self::Tuple(fields.iter().map(Self::from_ty).collect()),
            Ty::Array { inner, len } => Self::Array {
                inner: Box::new(Self::from_ty(inner)),
                len: len.clone(),
            },
            Ty::Slice(inner) => Self::Slice(Box::new(Self::from_ty(inner))),
            Ty::Reference { mutability, inner } => Self::Reference {
                mutability: *mutability,
                inner: Box::new(Self::from_ty(inner)),
            },
            Ty::Opaque { bounds } => Self::Opaque {
                bounds: bounds
                    .iter()
                    .map(InferOpaqueTraitBound::from_bound)
                    .collect(),
            },
            Ty::Syntax(ty) => Self::Syntax(Box::new(ty.clone())),
            Ty::Nominal(types) => {
                Self::Nominal(types.iter().map(InferNominalTy::from_nominal_ty).collect())
            }
            Ty::SelfTy(types) => {
                Self::SelfTy(types.iter().map(InferNominalTy::from_nominal_ty).collect())
            }
            Ty::Unknown => Self::Unknown,
        }
    }

    fn var_id(&self) -> Option<InferVarId> {
        match self {
            Self::Var(id) | Self::IntegerVar(id) | Self::FloatVar(id) => Some(*id),
            Self::Unit
            | Self::Never
            | Self::Primitive(_)
            | Self::Tuple(_)
            | Self::Array { .. }
            | Self::Slice(_)
            | Self::Reference { .. }
            | Self::Opaque { .. }
            | Self::Syntax(_)
            | Self::Nominal(_)
            | Self::SelfTy(_)
            | Self::Unknown => None,
        }
    }

    fn var_for_kind(kind: InferVarKind, id: InferVarId) -> Self {
        match kind {
            InferVarKind::Type => Self::Var(id),
            InferVarKind::Integer => Self::IntegerVar(id),
            InferVarKind::Float => Self::FloatVar(id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InferNominalTy {
    def: TypeDefRef,
    args: Vec<InferGenericArg>,
}

impl InferNominalTy {
    fn from_nominal_ty(ty: &NominalTy) -> Self {
        Self {
            def: ty.def,
            args: ty.args.iter().map(InferGenericArg::from_arg).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InferOpaqueTraitBound {
    trait_ref: TraitRef,
    args: Vec<InferGenericArg>,
}

impl InferOpaqueTraitBound {
    fn from_bound(bound: &OpaqueTraitBound) -> Self {
        Self {
            trait_ref: bound.trait_ref,
            args: bound.args.iter().map(InferGenericArg::from_arg).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum InferGenericArg {
    Type(Box<InferTy>),
    Lifetime(String),
    Const(String),
    FnTraitArgs {
        params: Vec<InferTy>,
        ret: Box<InferTy>,
    },
    AssocType {
        name: Name,
        ty: Option<Box<InferTy>>,
    },
    Unsupported(String),
}

impl InferGenericArg {
    fn from_arg(arg: &GenericArg) -> Self {
        match arg {
            GenericArg::Type(ty) => Self::Type(Box::new(InferTy::from_ty(ty))),
            GenericArg::Lifetime(lifetime) => Self::Lifetime(lifetime.clone()),
            GenericArg::Const(value) => Self::Const(value.clone()),
            GenericArg::FnTraitArgs { params, ret } => Self::FnTraitArgs {
                params: params.iter().map(InferTy::from_ty).collect(),
                ret: Box::new(InferTy::from_ty(ret)),
            },
            GenericArg::AssocType { name, ty } => Self::AssocType {
                name: name.clone(),
                ty: ty
                    .as_ref()
                    .map(|ty| Box::new(InferTy::from_ty(ty.as_ref()))),
            },
            GenericArg::Unsupported(text) => Self::Unsupported(text.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use rg_ir_model::{
        DefMapRef, StructId, TargetRef, TypeDefId, TypeDefRef,
        items::{FloatTy, SignedIntTy, UnsignedIntTy},
    };
    use rg_parse::TargetId;

    use super::*;

    fn type_def(index: usize) -> TypeDefRef {
        TypeDefRef {
            origin: DefMapRef::Target(TargetRef {
                package: rg_def_map::PackageSlot(0),
                target: TargetId(0),
            }),
            id: TypeDefId::Struct(StructId(index)),
        }
    }

    fn user_ty() -> Ty {
        Ty::nominal([NominalTy::bare(type_def(0))].into_iter().collect())
    }

    fn project_ty() -> Ty {
        Ty::nominal([NominalTy::bare(type_def(1))].into_iter().collect())
    }

    fn vec_ty(inner: InferTy) -> InferTy {
        InferTy::Nominal(
            [InferNominalTy {
                def: type_def(10),
                args: vec![InferGenericArg::Type(Box::new(inner))],
            }]
            .into_iter()
            .collect(),
        )
    }

    #[test]
    fn creates_body_inference_context_with_body_sized_slots() {
        let mut context = BodyInferenceCtx::new(2, 3);

        let var = context.table_mut().new_type_var();

        assert_eq!(context.expr_tys, vec![InferTy::Unknown; 2]);
        assert_eq!(context.binding_tys, vec![InferTy::Unknown; 3]);
        assert_eq!(context.table().finalize(&var), Ty::Unknown);
    }

    #[test]
    fn finalizes_unsolved_variables_to_stable_fallbacks() {
        let mut table = InferenceTable::new();

        let ty_var = table.new_type_var();
        let int_var = table.new_integer_var();
        let float_var = table.new_float_var();

        assert_eq!(table.finalize(&ty_var), Ty::Unknown);
        assert_eq!(
            table.finalize(&int_var),
            Ty::Primitive(PrimitiveTy::SignedInt(SignedIntTy::I32))
        );
        assert_eq!(
            table.finalize(&float_var),
            Ty::Primitive(PrimitiveTy::Float(FloatTy::F64))
        );
    }

    #[test]
    fn finalizes_solved_variables_inside_nominal_containers() {
        let mut table = InferenceTable::new();
        let element = table.new_type_var();

        assert!(table.unify(&element, &InferTy::from_ty(&user_ty())));

        assert_eq!(
            table.finalize(&vec_ty(element)),
            Ty::nominal(
                [NominalTy {
                    def: type_def(10),
                    args: vec![GenericArg::Type(Box::new(user_ty()))],
                }]
                .into_iter()
                .collect()
            )
        );
    }

    #[test]
    fn unifies_same_definition_nominal_generic_arguments() {
        let mut table = InferenceTable::new();
        let element = table.new_type_var();

        assert!(table.unify(
            &vec_ty(element.clone()),
            &vec_ty(InferTy::from_ty(&user_ty()))
        ));

        assert_eq!(
            table.finalize(&element),
            Ty::nominal([NominalTy::bare(type_def(0))].into_iter().collect())
        );
    }

    #[test]
    fn conflicting_variables_finalize_to_unknown() {
        let mut table = InferenceTable::new();
        let var = table.new_type_var();

        assert!(table.unify(&var, &InferTy::from_ty(&user_ty())));
        assert!(table.unify(&var, &InferTy::from_ty(&project_ty())));

        assert_eq!(table.finalize(&var), Ty::Unknown);
    }

    #[test]
    fn unknown_does_not_solve_variables() {
        let mut table = InferenceTable::new();
        let var = table.new_type_var();

        assert!(!table.unify(&var, &InferTy::Unknown));

        assert_eq!(table.finalize(&var), Ty::Unknown);
    }

    #[test]
    fn numeric_variables_accept_matching_primitive_evidence() {
        let mut table = InferenceTable::new();
        let int_var = table.new_integer_var();
        let float_var = table.new_float_var();

        assert!(table.unify(
            &int_var,
            &InferTy::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
        ));
        assert!(table.unify(
            &float_var,
            &InferTy::Primitive(PrimitiveTy::Float(FloatTy::F32))
        ));

        assert_eq!(
            table.finalize(&int_var),
            Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
        );
        assert_eq!(
            table.finalize(&float_var),
            Ty::Primitive(PrimitiveTy::Float(FloatTy::F32))
        );
    }

    #[test]
    fn numeric_variables_follow_already_solved_type_variables() {
        let mut table = InferenceTable::new();
        let type_var = table.new_type_var();
        let int_var = table.new_integer_var();

        assert!(table.unify(
            &type_var,
            &InferTy::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
        ));
        assert!(table.unify(&int_var, &type_var));

        assert_eq!(
            table.finalize(&int_var),
            Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
        );
        assert_eq!(
            table.finalize(&type_var),
            Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
        );
    }
}
