use std::fmt;

use rg_std::{MemorySize, Shrink};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

/// User-visible field identity shared by item declarations and body field syntax.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum FieldKey {
    Named(Name),
    Tuple(usize),
}

impl FieldKey {
    pub fn declaration_label(&self) -> String {
        match self {
            Self::Named(name) => name.to_string(),
            Self::Tuple(index) => format!("#{index}"),
        }
    }
}

impl fmt::Display for FieldKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Named(name) => write!(f, "{name}"),
            Self::Tuple(index) => write!(f, "{index}"),
        }
    }
}
