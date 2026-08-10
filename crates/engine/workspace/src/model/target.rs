use rg_std::{MemorySize, Shrink};
use std::path::PathBuf;
use wincode::{SchemaRead, SchemaWrite};

/// Normalized target metadata with one target kind per target.
#[derive(Debug, Clone, PartialEq, Eq, MemorySize)]
pub struct CargoTarget {
    pub name: String,
    pub kind: TargetKind,
    pub src_path: PathBuf,
}

/// Analysis-relevant target kinds.
///
/// Analysis recognizes a small set of target kinds directly. Unknown or less common kinds are kept
/// as stable display strings instead of becoming special model variants.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    derive_more::Display,
    SchemaRead,
    SchemaWrite,
    MemorySize,
    Shrink,
)]
pub enum TargetKind {
    #[display("lib")]
    Lib,
    #[display("proc-macro")]
    ProcMacro,
    #[display("bin")]
    Bin,
    #[display("example")]
    Example,
    #[display("test")]
    Test,
    #[display("bench")]
    Bench,
    #[display("custom-build")]
    CustomBuild,
    #[display("{_0}")]
    Other(String),
}

impl TargetKind {
    pub fn is_lib(&self) -> bool {
        matches!(self, Self::Lib | Self::ProcMacro)
    }

    pub fn is_custom_build(&self) -> bool {
        matches!(self, Self::CustomBuild)
    }

    pub fn is_proc_macro(&self) -> bool {
        matches!(self, Self::ProcMacro)
    }

    /// Cargo enables `cfg(test)` for test-like targets without reporting it in rustc cfg output.
    pub fn enables_test_cfg(&self) -> bool {
        matches!(self, Self::Test | Self::Bench)
    }

    // Used for predictable ordering, e.g.
    // in test snapshots.
    pub fn sort_order(&self) -> u8 {
        match self {
            Self::Lib => 0,
            Self::ProcMacro => 1,
            Self::Bin => 2,
            Self::Example => 3,
            Self::Test => 4,
            Self::Bench => 5,
            Self::CustomBuild => 6,
            Self::Other(_) => 7,
        }
    }
}
