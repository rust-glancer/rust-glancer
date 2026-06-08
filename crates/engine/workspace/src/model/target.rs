use std::path::PathBuf;

/// Normalized target metadata with one target kind per target.
#[derive(Debug, Clone, PartialEq, Eq, rg_memsize::MemorySize)]
pub struct Target {
    pub name: String,
    pub kind: TargetKind,
    pub src_path: PathBuf,
}

/// Analysis-relevant target kinds.
///
/// Analysis recognizes a small set of target kinds directly. Unknown or less common kinds are kept
/// as stable display strings instead of becoming special model variants.
#[derive(Debug, Clone, PartialEq, Eq, Hash, derive_more::Display, rg_memsize::MemorySize)]
pub enum TargetKind {
    #[display("lib")]
    Lib,
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
        matches!(self, Self::Lib)
    }

    pub fn is_custom_build(&self) -> bool {
        matches!(self, Self::CustomBuild)
    }

    // Used for predictable ordering, e.g.
    // in test snapshots.
    pub fn sort_order(&self) -> u8 {
        match self {
            Self::Lib => 0,
            Self::Bin => 1,
            Self::Example => 2,
            Self::Test => 3,
            Self::Bench => 4,
            Self::CustomBuild => 5,
            Self::Other(_) => 6,
        }
    }
}
