use wincode::{SchemaRead, SchemaWrite};

use rg_memsize::MemorySize;

use crate::{
    BindingId, ExprId, PatId, ScopeId,
    items::{ItemTreeId, TypeRef},
};

use super::BodySource;

/// One lowered statement.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct StmtData {
    pub source: BodySource,
    pub kind: StmtKind,
}

/// Statement forms that matter for the first Body IR pass.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub enum StmtKind {
    /// `let <pat>: Type = <expr>;` or `let <pat> = <expr> else { ... };`.
    Let {
        scope: ScopeId,
        pat: Option<PatId>,
        bindings: Vec<BindingId>,
        annotation: Option<TypeRef>,
        initializer: Option<ExprId>,
        else_branch: Option<ExprId>,
    },
    /// `<expr>;` or an expression statement without a semicolon.
    Expr { expr: ExprId, has_semicolon: bool },
    /// A block-local item represented in the body source-item arena.
    Item { item: ItemTreeId },
    /// An item statement that Body IR intentionally could not lower into the source-item arena.
    ItemIgnored,
}

impl StmtData {
    pub fn shrink_to_fit(&mut self) {
        self.kind.shrink_to_fit();
    }
}

impl StmtKind {
    fn shrink_to_fit(&mut self) {
        match self {
            Self::Let {
                bindings,
                annotation,
                ..
            } => {
                bindings.shrink_to_fit();
                if let Some(annotation) = annotation {
                    annotation.shrink_to_fit();
                }
            }
            Self::Expr { .. } | Self::Item { .. } | Self::ItemIgnored => {}
        }
    }
}
