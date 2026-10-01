use rg_ir_model::{
    BodyRef, DefMapRef, FieldKey, ModuleRef, Path, Span,
    identity::{DeclarationRef, ExprRef, FunctionBodyRef, LexicalScopeRef},
};
use rg_ir_view::source::{IndexedTypePath, IndexedTypePathScope};

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
        type_path: IndexedTypePath,
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

impl SymbolAt {
    /// Keep the source body's identity even when the symbol resolves to a crate-level item.
    /// Signature paths on body-local items get that identity from their declaring module.
    pub(crate) fn body_ref(&self) -> Option<BodyRef> {
        let origin = match self {
            Self::FunctionBody { body } => DefMapRef::Body(body.body_ir()),
            Self::Declaration { declaration, .. } => declaration.origin(),
            Self::Expr { expr } => DefMapRef::Body(expr.body_ir()),
            Self::TypePath { type_path, .. } => match type_path.scope() {
                IndexedTypePathScope::Body(scope) => DefMapRef::Body(scope.body_ir()),
                IndexedTypePathScope::Signature(scope) => scope.context().module.origin,
            },
            Self::ValuePath { scope, .. } | Self::RecordField { scope, .. } => {
                DefMapRef::Body(scope.body_ir())
            }
            Self::UsePath { module, .. } => module.origin,
        };
        match origin {
            DefMapRef::Body(body) => Some(body),
            DefMapRef::Crate(_) => None,
        }
    }
}
