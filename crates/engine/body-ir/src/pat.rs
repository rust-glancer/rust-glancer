use rg_item_tree::FieldKey;

use crate::{
    body::BodySource,
    ids::{BindingId, PatId},
    path::BodyPath,
};

/// One lowered pattern node.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum PatKind {
    Binding {
        binding: Option<BindingId>,
        subpat: Option<PatId>,
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
        fields: Vec<RecordPatField>,
    },
    Or {
        pats: Vec<PatId>,
    },
    Slice {
        fields: Vec<PatId>,
    },
    Ref {
        pat: PatId,
    },
    Box {
        pat: PatId,
    },
    Path {
        path: Option<BodyPath>,
    },
    Wildcard,
    Unsupported,
}

/// One field inside a record pattern.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct RecordPatField {
    pub key: FieldKey,
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
            Self::Record { path, fields } => {
                if let Some(path) = path {
                    path.shrink_to_fit();
                }
                fields.shrink_to_fit();
                for field in fields {
                    field.shrink_to_fit();
                }
            }
            Self::Or { pats } => pats.shrink_to_fit(),
            Self::Path { path } => {
                if let Some(path) = path {
                    path.shrink_to_fit();
                }
            }
            Self::Binding { .. }
            | Self::Ref { .. }
            | Self::Box { .. }
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
