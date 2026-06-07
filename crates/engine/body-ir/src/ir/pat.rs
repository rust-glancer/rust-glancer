use wincode::{SchemaRead, SchemaWrite};

use rg_ir_model::{
    BindingId, BodyPath, BodySource, ExprId, PatBindingMode, PatId, PatMutability, PatRangeKind,
};
use rg_item_tree::FieldKey;
use rg_memsize::MemorySize;

use crate::RecordFieldSyntax;

use super::expr::LiteralKind;

/// One lowered pattern node.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
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
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
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
    ConstBlock {
        #[memsize(scope = "expr")]
        expr: Option<ExprId>,
    },
    /// `_`.
    Wildcard,
    /// Pattern syntax that Body IR does not model directly.
    Unsupported,
}

/// One field inside a record pattern.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct RecordPatField {
    pub key: FieldKey,
    pub key_span: rg_parse::Span,
    pub source_span: rg_parse::Span,
    pub syntax: RecordFieldSyntax,
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
