use rg_item_tree::FieldKey;

use super::{
    body::BodySource,
    expr::LiteralKind,
    ids::{BindingId, ExprId, PatId},
    path::BodyPath,
};

/// One lowered pattern node.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct PatData {
    pub source: BodySource,
    pub kind: PatKind,
}

impl PatData {
    pub(crate) fn shrink_to_fit(&mut self) {
        self.kind.shrink_to_fit();
    }
}

/// Pattern forms that matter for binding and enum-payload type propagation.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum PatKind {
    Binding {
        mode: PatBindingMode,
        binding: Option<BindingId>,
        subpat: Option<PatId>,
        path: Option<BodyPath>,
    },
    Tuple {
        fields: Vec<PatId>,
    },
    TupleStruct {
        path: Option<BodyPath>,
        fields: Vec<PatId>,
    },
    Record {
        path: Option<BodyPath>,
        field_list_span: Option<rg_parse::Span>,
        fields: Vec<RecordPatField>,
        rest: Option<PatId>,
    },
    Or {
        pats: Vec<PatId>,
    },
    Slice {
        fields: Vec<PatId>,
    },
    Ref {
        mutability: PatMutability,
        pat: PatId,
    },
    Box {
        pat: PatId,
    },
    Path {
        path: Option<BodyPath>,
    },
    Rest,
    Literal {
        kind: LiteralKind,
        negated: bool,
    },
    Range {
        start: Option<PatId>,
        end: Option<PatId>,
        kind: Option<PatRangeKind>,
    },
    ConstBlock {
        expr: Option<ExprId>,
    },
    Wildcard,
    Unsupported,
}

/// Binding mode written on an identifier pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct PatBindingMode {
    pub by_ref: bool,
    pub mutable: bool,
}

impl PatBindingMode {
    pub const DEFAULT: Self = Self {
        by_ref: false,
        mutable: false,
    };
}

/// Mutability written on a reference pattern.
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
pub enum PatMutability {
    #[display("shared")]
    Shared,
    #[display("mut")]
    Mut,
}

/// Range operator written in a range pattern.
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
pub enum PatRangeKind {
    #[display("..")]
    Exclusive,
    #[display("..=")]
    Inclusive,
}

/// One field inside a record pattern.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct RecordPatField {
    pub key: FieldKey,
    pub key_span: rg_parse::Span,
    pub source_span: rg_parse::Span,
    pub pat: PatId,
}

impl PatKind {
    fn shrink_to_fit(&mut self) {
        match self {
            Self::Tuple { fields } | Self::Slice { fields } => fields.shrink_to_fit(),
            Self::TupleStruct { path, fields } => {
                if let Some(path) = path {
                    path.shrink_to_fit();
                }
                fields.shrink_to_fit();
            }
            Self::Record {
                path, fields, rest, ..
            } => {
                if let Some(path) = path {
                    path.shrink_to_fit();
                }
                fields.shrink_to_fit();
                for field in fields {
                    field.shrink_to_fit();
                }
                let _ = rest;
            }
            Self::Or { pats } => pats.shrink_to_fit(),
            Self::Path { path } => {
                if let Some(path) = path {
                    path.shrink_to_fit();
                }
            }
            Self::Binding { path, .. } => {
                if let Some(path) = path {
                    path.shrink_to_fit();
                }
            }
            Self::Ref { .. }
            | Self::Box { .. }
            | Self::Range { .. }
            | Self::ConstBlock { .. }
            | Self::Rest
            | Self::Literal { .. }
            | Self::Wildcard
            | Self::Unsupported => {}
        }
    }
}

impl RecordPatField {
    fn shrink_to_fit(&mut self) {
        self.key.shrink_to_fit();
    }
}
