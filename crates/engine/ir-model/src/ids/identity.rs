//! Query-facing identities composed from the indexed IR storage refs.
//!
//! Most declarations now use the semantic-shaped refs directly. This module only keeps aggregate
//! identities that do not exist in a single storage layer: source declarations, expressions, and
//! lexical scopes.

use std::fmt;

use crate::{
    BodyBindingRef, BodyRef as BodyIrBodyRef, ConstRef, DefId, EnumVariantRef, ExprId, FieldRef,
    FunctionRef, ImplRef, LocalDefRef, ModuleRef, ScopeId, SemanticItemRef, StaticRef, TargetRef,
    TraitRef, TypeAliasRef, TypeDefRef,
};

/// Stable identity for one lowered function body.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionBodyRef(BodyIrBodyRef);

impl FunctionBodyRef {
    pub fn body_ir(self) -> BodyIrBodyRef {
        self.0
    }

    pub fn from_body_ir(body: BodyIrBodyRef) -> Self {
        Self(body)
    }

    pub fn target(self) -> TargetRef {
        self.0.target
    }
}

impl fmt::Debug for FunctionBodyRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FunctionBodyRef")
            .field("target", &self.0.target)
            .field("body", &self.0.body)
            .finish()
    }
}

/// Stable identity for one expression inside a lowered body.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExprRef {
    body: BodyIrBodyRef,
    expr: ExprId,
}

impl ExprRef {
    pub fn new(body: BodyIrBodyRef, expr: ExprId) -> Self {
        Self { body, expr }
    }

    pub fn body_ir(self) -> BodyIrBodyRef {
        self.body
    }

    pub fn expr_id(self) -> ExprId {
        self.expr
    }
}

impl fmt::Debug for ExprRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExprRef")
            .field("body", &FunctionBodyRef::from_body_ir(self.body))
            .field("expr", &self.expr)
            .finish()
    }
}

/// Stable identity for one lexical scope inside a lowered body.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct LexicalScopeRef {
    body: BodyIrBodyRef,
    scope: ScopeId,
}

impl LexicalScopeRef {
    pub fn new(body: BodyIrBodyRef, scope: ScopeId) -> Self {
        Self { body, scope }
    }

    pub fn body_ir(self) -> BodyIrBodyRef {
        self.body
    }

    pub fn scope_id(self) -> ScopeId {
        self.scope
    }
}

impl fmt::Debug for LexicalScopeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LexicalScopeRef")
            .field("body", &FunctionBodyRef::from_body_ir(self.body))
            .field("scope", &self.scope)
            .finish()
    }
}

/// Stable declaration identity exposed by indexed-data views.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    derive_more::From,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub enum DeclarationRef {
    #[from]
    Module(ModuleRef),
    #[from]
    LocalDef(LocalDefRef),
    #[from(
        SemanticItemRef,
        TypeDefRef,
        TraitRef,
        ImplRef,
        FunctionRef,
        TypeAliasRef,
        ConstRef,
        StaticRef
    )]
    Item(SemanticItemRef),
    #[from]
    Field(FieldRef),
    #[from]
    EnumVariant(EnumVariantRef),
    #[from]
    BodyBinding(BodyBindingRef),
}

impl DeclarationRef {
    pub fn module(module: ModuleRef) -> Self {
        Self::Module(module)
    }

    pub fn local_def(local_def: LocalDefRef) -> Self {
        Self::LocalDef(local_def)
    }

    pub fn from_def(def: DefId) -> Self {
        match def {
            DefId::Module(module) => Self::Module(module),
            DefId::Local(local_def) => Self::LocalDef(local_def),
        }
    }

    pub fn body_binding(binding: BodyBindingRef) -> Self {
        Self::BodyBinding(binding)
    }
}

impl From<DefId> for DeclarationRef {
    fn from(def: DefId) -> Self {
        Self::from_def(def)
    }
}

impl fmt::Debug for DeclarationRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Module(module) => f
                .debug_struct("DeclarationRef")
                .field("kind", &"module")
                .field("module", &module)
                .finish(),
            Self::LocalDef(local_def) => f
                .debug_struct("DeclarationRef")
                .field("kind", &"local_def")
                .field("local_def", &local_def)
                .finish(),
            Self::Item(item) => f
                .debug_struct("DeclarationRef")
                .field("kind", &"item")
                .field("item", &item)
                .finish(),
            Self::Field(field) => f
                .debug_struct("DeclarationRef")
                .field("kind", &"field")
                .field("field", &field)
                .finish(),
            Self::EnumVariant(variant) => f
                .debug_struct("DeclarationRef")
                .field("kind", &"enum_variant")
                .field("variant", &variant)
                .finish(),
            Self::BodyBinding(binding) => f
                .debug_struct("DeclarationRef")
                .field("kind", &"binding")
                .field("body", &FunctionBodyRef::from_body_ir(binding.body))
                .field("binding", &binding.binding)
                .finish(),
        }
    }
}
