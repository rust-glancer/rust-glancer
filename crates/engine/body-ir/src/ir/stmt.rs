use rg_ir_model::{BindingId, ExprId, PatId, ScopeId};
use rg_item_tree::{ItemTreeId, TypeRef};
use rg_parse::Span;
use rg_text::Name;
use rg_ty::Ty;

use super::body::BodySource;

/// One local binding introduced by a parameter or `let`.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct BindingData {
    pub source: BodySource,
    pub name_span: Option<Span>,
    pub scope: ScopeId,
    pub kind: BindingKind,
    pub name: Option<Name>,
    pub annotation: Option<TypeRef>,
    pub ty: Ty,
}

impl BindingData {
    pub(crate) fn shrink_to_fit(&mut self) {
        if let Some(name) = &mut self.name {
            name.shrink_to_fit();
        }
        if let Some(annotation) = &mut self.annotation {
            annotation.shrink_to_fit();
        }
        self.ty.shrink_to_fit();
    }
}

/// Local binding category.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub enum BindingKind {
    /// `param` in `fn f(param: Type)`.
    #[display("param")]
    Param,
    /// `self`, `&self`, or another receiver parameter.
    #[display("self_param")]
    SelfParam(BodySelfParamKind),
    /// `let name = value`.
    #[display("let")]
    Let,
}

/// Receiver form written by a function's self parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum BodySelfParamKind {
    Value,
    Reference { mutability: rg_ty::RefMutability },
    Explicit,
}

/// One lowered statement.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct StmtData {
    pub source: BodySource,
    pub kind: StmtKind,
}

impl StmtData {
    pub(crate) fn shrink_to_fit(&mut self) {
        self.kind.shrink_to_fit();
    }
}

/// Statement forms that matter for the first Body IR pass.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
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
