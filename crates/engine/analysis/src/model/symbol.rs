use std::fmt;

use rg_ir_model::{
    ModuleRef,
    identity::{DeclarationRef, ExprRef, FunctionBodyRef, LexicalScopeRef},
};
use rg_ir_storage::{Path, TypePathContext};
use rg_item_tree::FieldKey;
use rg_parse::Span;

/// Scope in which a type path should be resolved.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TypePathScopeRef(TypePathScopeRepr);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypePathScopeRepr {
    Signature(TypePathContext),
    Body(LexicalScopeRef),
}

impl TypePathScopeRef {
    pub(crate) fn signature(context: TypePathContext) -> Self {
        Self(TypePathScopeRepr::Signature(context))
    }

    pub(crate) fn body(scope: LexicalScopeRef) -> Self {
        Self(TypePathScopeRepr::Body(scope))
    }

    pub(crate) fn repr(self) -> TypePathScopeRepr {
        self.0
    }
}

impl fmt::Debug for TypePathScopeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            TypePathScopeRepr::Signature(context) => f
                .debug_struct("TypePathScopeRef")
                .field("kind", &"signature")
                .field("module", &context.module)
                .finish(),
            TypePathScopeRepr::Body(scope) => f
                .debug_struct("TypePathScopeRef")
                .field("kind", &"body")
                .field("scope", &scope)
                .finish(),
        }
    }
}

/// Symbol found at one source offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolAt {
    /// Function body declaration, e.g. the name in `fn use_it() { ... }`.
    FunctionBody { body: FunctionBodyRef },
    /// Declaration-like source node.
    Declaration {
        declaration: DeclarationRef,
        span: Span,
    },
    /// Lowered expression node, e.g. the whole `user.id()` call expression.
    Expr { expr: ExprRef },
    /// Type-namespace path, e.g. `User` in a signature or `let user: User;`.
    TypePath {
        scope: TypePathScopeRef,
        path: Path,
        span: Span,
    },
    /// Value-namespace path inside a lowered body.
    ValuePath {
        scope: LexicalScopeRef,
        path: Path,
        span: Span,
    },
    /// Field key inside an explicit record expression or pattern.
    RecordField {
        scope: LexicalScopeRef,
        owner: Path,
        key: FieldKey,
        span: Span,
    },
    /// Import path, e.g. `crate::user::User` in `use crate::user::User;`.
    UsePath {
        module: ModuleRef,
        path: Path,
        span: Span,
    },
}
