use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};
#[derive(
    Debug, Clone, PartialEq, Eq, derive_more::Display, SchemaRead, SchemaWrite, MemorySize, Shrink,
)]
pub enum VisibilityLevel {
    #[display("private")]
    Private,
    #[display("pub")]
    Public,
    #[display("pub(crate)")]
    Crate,
    #[display("pub(super)")]
    Super,
    #[display("pub(self)")]
    Self_,
    #[display("pub(in {_0})")]
    Restricted(String),
    #[display("{_0}")]
    Unknown(String),
}
