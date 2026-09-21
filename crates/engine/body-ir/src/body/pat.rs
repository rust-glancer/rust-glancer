use rg_ir_model::{BindingId, ExprId, FieldKey, Mutability, PatId, Span};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use super::{BodyPath, BodySource, LiteralKind, RecordFieldSyntax};

/// Binding mode written on an identifier pattern.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink,
)]
#[memsize(leaf)]
#[shrink(leaf)]
pub struct PatBindingMode {
    pub by_ref: bool,
    pub mutable: bool,
}

/// Range operator written in a range pattern.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    SchemaRead,
    SchemaWrite,
    MemorySize,
    Shrink,
)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum PatRangeKind {
    /// `..`.
    #[display("..")]
    Exclusive,
    /// `..=`.
    #[display("..=")]
    Inclusive,
}

/// One lowered pattern node.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct PatData {
    pub source: BodySource,
    pub kind: PatKind,
}

/// Pattern forms that matter for binding and enum-payload type propagation.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
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
        field_list_span: Option<Span>,
        fields: Vec<RecordPatField>,
        rest: Option<PatId>,
    },
    /// `<pat> | <pat>`.
    Or { pats: Vec<PatId> },
    /// `[<pat>, ...]`.
    Slice { fields: Vec<PatId> },
    /// `&<pat>` or `&mut <pat>`.
    Ref { mutability: Mutability, pat: PatId },
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

impl PatKind {
    /// Direct child patterns in source order. Callers handle const-block expressions separately.
    pub fn child_pats(&self) -> impl Iterator<Item = PatId> + '_ {
        // Borrow the existing child lists and keep the few standalone ids inline. Recursive
        // walkers can then use one iterator without allocating a new list for each pattern.
        let mut pats: &[PatId] = &[];
        let mut record_fields: &[RecordPatField] = &[];
        let mut trailing = [None; 2];
        match self {
            Self::Binding { subpat, .. } => pats = subpat.as_slice(),
            Self::Tuple { fields }
            | Self::TupleStruct { fields, .. }
            | Self::Or { pats: fields }
            | Self::Slice { fields } => pats = fields,
            Self::Record { fields, rest, .. } => {
                record_fields = fields;
                trailing[0] = *rest;
            }
            Self::Ref { pat, .. } | Self::Box { pat } => pats = std::slice::from_ref(pat),
            Self::Range { start, end, .. } => trailing = [*start, *end],
            Self::Path { .. }
            | Self::Rest
            | Self::Literal { .. }
            | Self::ConstBlock { .. }
            | Self::Wildcard
            | Self::Unsupported => {}
        }
        pats.iter()
            .copied()
            .chain(record_fields.iter().map(|field| field.pat))
            .chain(trailing.into_iter().flatten())
    }

    /// Returns any path syntactically owned by this pattern node.
    pub fn path(&self) -> Option<&BodyPath> {
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
    ///
    /// Record patterns resolve their constructor through the type namespace and are exposed
    /// separately by [`Self::record_path`].
    pub fn value_path(&self) -> Option<&BodyPath> {
        match self {
            Self::TupleStruct { path, .. } | Self::Path { path } => path.as_ref(),
            Self::Binding { binding, path, .. } if binding.is_none() => path.as_ref(),
            Self::Binding { .. }
            | Self::Record { .. }
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

    /// Returns the type-namespace constructor path owned by a record pattern.
    pub fn record_path(&self) -> Option<&BodyPath> {
        let Self::Record { path, .. } = self else {
            return None;
        };
        path.as_ref()
    }
}

/// One field inside a record pattern.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct RecordPatField {
    pub key: FieldKey,
    pub key_span: Span,
    pub source_span: Span,
    pub syntax: RecordFieldSyntax,
    pub pat: PatId,
}
