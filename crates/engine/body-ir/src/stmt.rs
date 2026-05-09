use rg_item_tree::TypeRef;
use rg_text::Name;

use crate::{
    body::BodySource,
    ids::{BindingId, BodyImplId, BodyItemId, ExprId, PatId, ScopeId},
    ty::BodyTy,
};

/// One local binding introduced by a parameter or `let`.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub enum BindingKind {
    #[display("param")]
    Param,
    #[display("self_param")]
    SelfParam,
    #[display("let")]
    Let,
}

/// One lowered statement.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum StmtKind {
    Let {
        scope: ScopeId,
        pat: Option<PatId>,
        bindings: Vec<BindingId>,
        annotation: Option<TypeRef>,
        initializer: Option<ExprId>,
    },
    Expr {
        expr: ExprId,
        has_semicolon: bool,
    },
    Item {
        item: BodyItemId,
    },
    Impl {
        impl_id: BodyImplId,
    },
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
            Self::Expr { .. } | Self::Item { .. } | Self::Impl { .. } | Self::ItemIgnored => {}
        }
    }
}
