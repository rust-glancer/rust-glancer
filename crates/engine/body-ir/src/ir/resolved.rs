use rg_std::{MemorySize, Shrink, UniqueVec};
use wincode::{SchemaRead, SchemaWrite};

use rg_arena::Arena;
use rg_ir_model::{
    BindingId, BodyBindingRef, BodyRef, ExprId, FunctionRef, identity::DeclarationRef,
};
use rg_ty::{GenericArgs, Ty};

use super::body::{BodyData, ExprKind};

/// Persisted semantic sidecar for one frozen [`BodyData`].
///
/// `BodyData` owns the syntax-shaped structure. This type owns the conclusions derived from that
/// structure: binding types, expression types and resolutions, and selected call targets. The
/// dense binding and expression arenas deliberately mirror the ids and cardinalities in the body,
/// so one `ExprId` addresses both its structural node and its semantic facts. Calls are kept sparse
/// because only call expressions can have [`CallFacts`].
///
/// Resolution creates this sidecar only after structural lowering and binding compaction finish.
/// Readers normally pair it with its body through `BodyView`; it is not another owning body model.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodyFacts {
    pub(crate) bindings: Arena<BindingId, BindingFacts>,
    pub(crate) exprs: Arena<ExprId, ExprFacts>,
    /// Calls are sparse relative to expressions, so selected targets do not need one dense option
    /// slot per expression.
    calls: Vec<(ExprId, CallFacts)>,
}

impl BodyFacts {
    /// Allocate dense fact sidecars for a finalized structural body.
    ///
    /// Resolution fills these slots without changing their identity or cardinality. Structural
    /// build steps, including ambiguous pattern binding compaction, have already finished.
    pub(crate) fn for_body(body: &BodyData) -> Self {
        Self {
            bindings: Arena::from_vec(
                body.bindings()
                    .iter()
                    .map(|_| BindingFacts::default())
                    .collect(),
            ),
            exprs: Arena::from_vec(body.exprs().iter().map(|_| ExprFacts::default()).collect()),
            calls: Vec::new(),
        }
    }

    /// Check that this sidecar can be indexed with the ids defined by `body`.
    ///
    /// Dense binding and expression facts must have the same lengths as their structural arenas.
    /// Sparse call facts must be ordered, unique, and attached only to call-shaped expressions;
    /// that ordering is what makes `call` a binary search instead of a body-wide scan.
    ///
    /// This validates storage shape, including after deserialization. It deliberately does not try
    /// to prove that an inferred type or selected target is semantically correct.
    pub(crate) fn is_aligned_with(&self, body: &BodyData) -> bool {
        let mut previous_call = None;
        let calls_are_aligned = self.calls.iter().all(|(expr, _)| {
            let ordered = previous_call
                .replace(expr.0)
                .is_none_or(|previous| previous < expr.0);
            let is_call = body.expr(*expr).is_some_and(|data| {
                matches!(
                    data.kind,
                    ExprKind::Call { .. } | ExprKind::MethodCall { .. }
                )
            });
            ordered && is_call
        });

        self.bindings.len() == body.bindings().len()
            && self.exprs.len() == body.exprs().len()
            && calls_are_aligned
    }

    pub(crate) fn set_expr_ty(&mut self, expr: ExprId, ty: Ty) {
        self.exprs[expr].ty = ty;
    }

    pub(crate) fn set_expr_resolution(&mut self, expr: ExprId, resolution: BodyResolution) {
        self.exprs[expr].resolution = resolution;
    }

    pub(crate) fn set_binding_ty(&mut self, binding: BindingId, ty: Ty) {
        self.bindings[binding].ty = ty;
    }

    /// Replace sparse call facts in expression-id order.
    ///
    /// Inference finalization naturally walks its expression-indexed call slots in that order.
    /// Preserving it here enables allocation-free lookup by binary search.
    pub(crate) fn set_calls(&mut self, calls: Vec<(ExprId, CallFacts)>) {
        debug_assert!(calls.windows(2).all(|pair| pair[0].0.0 < pair[1].0.0));
        self.calls = calls;
    }

    pub(crate) fn call(&self, expr: ExprId) -> Option<&CallFacts> {
        let index = self
            .calls
            .binary_search_by_key(&expr.0, |(candidate, _)| candidate.0)
            .ok()?;
        Some(&self.calls[index].1)
    }
}

/// Finalized target and substitutions selected for one call expression.
///
/// The argument list follows the selected function's full canonical generic order, including
/// inherited impl or trait parameters. No inference variables cross this persistence boundary.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct CallFacts {
    function: FunctionRef,
    generic_args: GenericArgs,
}

impl CallFacts {
    pub(crate) fn new(function: FunctionRef, generic_args: GenericArgs) -> Self {
        Self {
            function,
            generic_args,
        }
    }

    pub fn function(&self) -> FunctionRef {
        self.function
    }

    pub fn generic_args(&self) -> &GenericArgs {
        &self.generic_args
    }
}

/// Resolved facts derived for one expression during body resolution.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ExprFacts {
    pub(crate) resolution: BodyResolution,
    pub ty: Ty,
}

impl Default for ExprFacts {
    fn default() -> Self {
        Self {
            resolution: BodyResolution::Unknown,
            ty: Ty::Unknown,
        }
    }
}

/// Resolved facts derived for one local binding during body resolution.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BindingFacts {
    pub ty: Ty,
}

impl Default for BindingFacts {
    fn default() -> Self {
        Self { ty: Ty::Unknown }
    }
}

/// Best-effort semantic resolution attached to body expressions.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub(crate) enum BodyResolution {
    /// Lexical value binding introduced by a pattern or parameter.
    Binding(BindingId),
    /// Item-like declarations, fields, enum variants, functions, consts, statics, or modules.
    Declarations(UniqueVec<DeclarationRef>),
    #[default]
    Unknown,
}

impl BodyResolution {
    pub(crate) fn declarations(&self, body_ref: BodyRef) -> Vec<DeclarationRef> {
        match self {
            Self::Binding(binding) => vec![DeclarationRef::body_binding(BodyBindingRef {
                body: body_ref,
                binding: *binding,
            })],
            Self::Declarations(declarations) => declarations.clone().into_vec(),
            Self::Unknown => Vec::new(),
        }
    }
}
