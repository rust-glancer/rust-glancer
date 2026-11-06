use clap::ValueEnum;
use rg_project::PackageResidencyPolicy;

/// Developer-only storage choices for the `analyze` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliPackageResidencyPolicy {
    AllResident,
    AllOffloadable,
}

impl From<CliPackageResidencyPolicy> for PackageResidencyPolicy {
    fn from(policy: CliPackageResidencyPolicy) -> Self {
        match policy {
            CliPackageResidencyPolicy::AllResident => Self::AllResident,
            CliPackageResidencyPolicy::AllOffloadable => Self::AllOffloadable,
        }
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
