#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    derive_more::Display,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub enum VisibilityLevel {
    #[display("private")]
    Private,
    #[display("pub")]
    Public,
    #[display("pub")]
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
