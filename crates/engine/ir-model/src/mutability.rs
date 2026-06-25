use std::fmt;

use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Reference mutability marker shared by type syntax and body forms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum Mutability {
    Shared,
    Mutable,
}

impl Mutability {
    pub fn from_mut_token(is_mut: bool) -> Self {
        if is_mut { Self::Mutable } else { Self::Shared }
    }

    pub fn render_prefix(self) -> &'static str {
        match self {
            Self::Shared => "&",
            Self::Mutable => "&mut ",
        }
    }
}

impl fmt::Display for Mutability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shared => write!(f, "shared"),
            Self::Mutable => write!(f, "mut"),
        }
    }
}
