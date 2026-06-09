use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Rust edition used by a package.
///
/// We keep this normalized so later phases can ask edition-shaped questions without depending on
/// an external transport model.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    derive_more::Display,
    SchemaRead,
    SchemaWrite,
    MemorySize,
    Shrink,
)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum RustEdition {
    #[display("2015")]
    Edition2015,
    #[display("2018")]
    Edition2018,
    #[display("2021")]
    Edition2021,
    #[display("2024")]
    Edition2024,
}

impl RustEdition {
    pub fn prelude_module(self) -> &'static str {
        match self {
            Self::Edition2015 => "rust_2015",
            Self::Edition2018 => "rust_2018",
            Self::Edition2021 => "rust_2021",
            Self::Edition2024 => "rust_2024",
        }
    }
}
