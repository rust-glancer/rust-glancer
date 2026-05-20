use rg_item_tree::TypeRef;
use rg_text::Name;

use super::{
    body::BodySource,
    ids::{
        BindingId, BodyFunctionId, BodyImplId, BodyItemId, BodyValueItemId, ExprId, PatId, ScopeId,
    },
    ty::BodyTy,
};

/// One local binding introduced by a parameter or `let`.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct BindingData {
    pub source: BodySource,
    pub scope: ScopeId,
    pub kind: BindingKind,
    pub name: Option<Name>,
    pub annotation: Option<TypeRef>,
    pub ty: BodyTy,
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
)]
pub enum BindingKind {
    /// `param` in `fn f(param: Type)`.
    #[display("param")]
    Param,
    /// `self`, `&self`, or another receiver parameter.
    #[display("self_param")]
    SelfParam,
    /// `let name = value`.
    #[display("let")]
    Let,
}

/// One lowered statement.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
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
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
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
    /// A block-local item kept in Body IR, such as `struct Local;`.
    Item { item: BodyItemId },
    /// A block-local value item kept in Body IR, such as `const LOCAL: u8 = 1;`.
    ValueItem { item: BodyValueItemId },
    /// A block-local function declaration.
    Function { function: BodyFunctionId },
    /// A block-local `impl`.
    Impl { impl_id: BodyImplId },
    /// An item statement that Body IR intentionally keeps only as source layout.
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
            Self::Expr { .. }
            | Self::Item { .. }
            | Self::ValueItem { .. }
            | Self::Function { .. }
            | Self::Impl { .. }
            | Self::ItemIgnored => {}
        }
    }
}
