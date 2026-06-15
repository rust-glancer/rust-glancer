use rg_ir_model::Path;
use rg_ir_model::items::FieldKey;
use rg_ir_model::{
    ModuleRef,
    identity::{DeclarationRef, ExprRef, FunctionBodyRef, LexicalScopeRef},
};
use rg_ir_view::source::IndexedTypePathScope;
use rg_parse::Span;

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
        scope: IndexedTypePathScope,
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
