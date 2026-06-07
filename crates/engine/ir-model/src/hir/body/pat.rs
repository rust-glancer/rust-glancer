use wincode::{SchemaRead, SchemaWrite};

use rg_memsize::MemorySize;

/// Binding mode written on an identifier pattern.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
#[memsize(leaf)]
pub struct PatBindingMode {
    pub by_ref: bool,
    pub mutable: bool,
}

/// Mutability written on a reference pattern.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, derive_more::Display, SchemaRead, SchemaWrite, MemorySize,
)]
#[memsize(leaf)]
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
    Debug, Clone, Copy, PartialEq, Eq, derive_more::Display, SchemaRead, SchemaWrite, MemorySize,
)]
#[memsize(leaf)]
pub enum PatRangeKind {
    /// `..`.
    #[display("..")]
    Exclusive,
    /// `..=`.
    #[display("..=")]
    Inclusive,
}
