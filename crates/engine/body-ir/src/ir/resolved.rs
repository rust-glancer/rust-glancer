use rg_std::{MemorySize, Shrink, UniqueVec};
use wincode::{SchemaRead, SchemaWrite};

use rg_arena::Arena;
use rg_ir_model::{BindingId, BodyBindingRef, BodyData, BodyRef, ExprId, identity::DeclarationRef};
use rg_ty::Ty;

/// Pass-derived facts for one resolved body.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodyFacts {
    pub(crate) bindings: Arena<BindingId, BindingFacts>,
    pub(crate) exprs: Arena<ExprId, ExprFacts>,
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
        }
    }

    pub(crate) fn is_aligned_with(&self, body: &BodyData) -> bool {
        self.bindings.len() == body.bindings().len() && self.exprs.len() == body.exprs().len()
    }

    pub(crate) fn set_expr_ty(&mut self, expr: ExprId, ty: Ty) {
        self.exprs[expr].ty = ty;
    }

    pub(crate) fn set_expr_resolution(&mut self, expr: ExprId, resolution: BodyResolution) {
        self.exprs[expr].resolution = resolution;
    }

    pub(crate) fn set_expr(&mut self, expr: ExprId, resolution: BodyResolution, ty: Ty) {
        let facts = &mut self.exprs[expr];
        facts.resolution = resolution;
        facts.ty = ty;
    }

    pub(crate) fn set_binding_ty(&mut self, binding: BindingId, ty: Ty) {
        self.bindings[binding].ty = ty;
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
