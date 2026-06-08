use wincode::{SchemaRead, SchemaWrite};

use rg_memsize::MemorySize;

/// Rust edition used by a package.
///
/// We keep this normalized instead of leaking `cargo_metadata::Edition` so later phases can ask
/// edition-shaped questions without depending on Cargo's transport model.
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
)]
#[memsize(leaf)]
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
    pub(crate) fn from_cargo(edition: cargo_metadata::Edition) -> Self {
        match edition {
            cargo_metadata::Edition::E2015 => Self::Edition2015,
            cargo_metadata::Edition::E2018 => Self::Edition2018,
            cargo_metadata::Edition::E2021 => Self::Edition2021,
            cargo_metadata::Edition::E2024 => Self::Edition2024,
            // Cargo parses a few future-edition placeholders. Until rust-src exposes matching
            // prelude modules, resolve them through the newest edition we understand.
            _ => Self::Edition2024,
        }
    }

    pub fn prelude_module(self) -> &'static str {
        match self {
            Self::Edition2015 => "rust_2015",
            Self::Edition2018 => "rust_2018",
            Self::Edition2021 => "rust_2021",
            Self::Edition2024 => "rust_2024",
        }
    }
}
