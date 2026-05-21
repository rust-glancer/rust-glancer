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
    /// `name`, `ref mut name`, or `name @ <pat>`.
    Binding {
        mode: PatBindingMode,
        binding: Option<BindingId>,
        subpat: Option<PatId>,
        path: Option<BodyPath>,
    },
    /// `(<pat>, ...)`.
    Tuple { fields: Vec<PatId> },
    /// `Path(<pat>, ...)`.
    TupleStruct {
        path: Option<BodyPath>,
        fields: Vec<PatId>,
    },
    /// `Path { field, other: <pat>, .. }`.
    Record {
        path: Option<BodyPath>,
        field_list_span: Option<rg_parse::Span>,
        fields: Vec<RecordPatField>,
        rest: Option<PatId>,
    },
    /// `<pat> | <pat>`.
    Or { pats: Vec<PatId> },
    /// `[<pat>, ...]`.
    Slice { fields: Vec<PatId> },
    /// `&<pat>` or `&mut <pat>`.
    Ref {
        mutability: PatMutability,
        pat: PatId,
    },
    /// `box <pat>`.
    Box { pat: PatId },
    /// `CONST`, `Enum::Variant`, or another path-only pattern.
    Path { path: Option<BodyPath> },
    /// `..`.
    Rest,
    /// `42`, `"text"`, `true`, or another literal token.
    Literal { kind: LiteralKind, negated: bool },
    /// `<start>..<end>`, `<start>..=<end>`, `..<end>`, or `<start>..`.
    Range {
        start: Option<PatId>,
        end: Option<PatId>,
        kind: Option<PatRangeKind>,
    },
    /// `const { ... }`.
    ConstBlock { expr: Option<ExprId> },
    /// `_`.
    Wildcard,
    /// Pattern syntax that Body IR does not model directly.
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
    /// `&<pat>`.
    #[display("shared")]
    Shared,
    /// `&mut <pat>`.
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
    /// `..`.
    #[display("..")]
    Exclusive,
    /// `..=`.
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
    /// Returns any path syntactically owned by this pattern node.
    pub(crate) fn path(&self) -> Option<&BodyPath> {
        match self {
            Self::TupleStruct { path, .. }
            | Self::Record { path, .. }
            | Self::Path { path }
            | Self::Binding { path, .. } => path.as_ref(),
            Self::Tuple { .. }
            | Self::Or { .. }
            | Self::Slice { .. }
            | Self::Ref { .. }
            | Self::Box { .. }
            | Self::Range { .. }
            | Self::ConstBlock { .. }
            | Self::Rest
            | Self::Literal { .. }
            | Self::Wildcard
            | Self::Unsupported => None,
        }
    }

    /// Returns the pattern path when it should behave as an editor-visible value path.
    pub(crate) fn value_path(&self) -> Option<&BodyPath> {
        match self {
            Self::TupleStruct { path, .. } | Self::Record { path, .. } | Self::Path { path } => {
                path.as_ref()
            }
            Self::Binding { binding, path, .. } if binding.is_none() => path.as_ref(),
            Self::Binding { .. }
            | Self::Tuple { .. }
            | Self::Or { .. }
            | Self::Slice { .. }
            | Self::Ref { .. }
            | Self::Box { .. }
            | Self::Range { .. }
            | Self::ConstBlock { .. }
            | Self::Rest
            | Self::Literal { .. }
            | Self::Wildcard
            | Self::Unsupported => None,
        }
    }

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
