use rg_std::MemorySize;
use wincode::{SchemaRead, SchemaWrite};
#[derive(
    Debug, Clone, PartialEq, Eq, derive_more::Display, SchemaRead, SchemaWrite, MemorySize,
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

impl VisibilityLevel {
    pub fn shrink_to_fit(&mut self) {
        match self {
            Self::Restricted(path) | Self::Unknown(path) => path.shrink_to_fit(),
            Self::Private | Self::Public | Self::Crate | Self::Super | Self::Self_ => {}
        }
    }
}
