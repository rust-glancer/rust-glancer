use std::fmt as std_fmt;

use clap::ValueEnum;
use rg_project::{IndexingPerformancePreference, PackageResidencyPolicy};

/// CLI-facing package residency names for the `analyze` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliPackageResidencyPolicy {
    AllResident,
    Workspace,
    WorkspaceAndPathDeps,
    WorkspacePathAndDirectDeps,
    AllOffloadable,
}

impl From<CliPackageResidencyPolicy> for PackageResidencyPolicy {
    fn from(policy: CliPackageResidencyPolicy) -> Self {
        match policy {
            CliPackageResidencyPolicy::AllResident => Self::AllResident,
            CliPackageResidencyPolicy::Workspace => Self::WorkspaceResident,
            CliPackageResidencyPolicy::WorkspaceAndPathDeps => Self::WorkspaceAndPathDepsResident,
            CliPackageResidencyPolicy::WorkspacePathAndDirectDeps => {
                Self::WorkspacePathAndDirectDepsResident
            }
            CliPackageResidencyPolicy::AllOffloadable => Self::AllOffloadable,
        }
    }
}

/// CLI-facing indexing preference names for the `analyze` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliIndexingPreference {
    LowerPeakMemory,
    FasterBuilds,
}

impl From<CliIndexingPreference> for IndexingPerformancePreference {
    fn from(preference: CliIndexingPreference) -> Self {
        match preference {
            CliIndexingPreference::LowerPeakMemory => Self::LowerPeakMemory,
            CliIndexingPreference::FasterBuilds => Self::FasterBuilds,
        }
    }
}

impl From<IndexingPerformancePreference> for CliIndexingPreference {
    fn from(preference: IndexingPerformancePreference) -> Self {
        match preference {
            IndexingPerformancePreference::LowerPeakMemory => Self::LowerPeakMemory,
            IndexingPerformancePreference::FasterBuilds => Self::FasterBuilds,
        }
    }
}

impl Default for CliIndexingPreference {
    fn default() -> Self {
        IndexingPerformancePreference::default().into()
    }
}

impl std_fmt::Display for CliIndexingPreference {
    fn fmt(&self, f: &mut std_fmt::Formatter<'_>) -> std_fmt::Result {
        f.write_str(IndexingPerformancePreference::from(*self).config_name())
    }
}

/// Output format for the `analyze` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum OutputFormat {
    Text,
    Json,
    RichJson,
    Html,
}
