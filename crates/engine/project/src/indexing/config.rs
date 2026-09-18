//! User-facing indexing trade-offs passed down to build phases.

use std::{
    fmt,
    num::{NonZeroUsize, ParseIntError},
    str::FromStr,
};

use rg_def_map::MacroExpansionPerformancePreference;
use rg_ir_model::PackageSlot;

use crate::PackageResidencyPlan;

const LOWER_PEAK_MEMORY_BODY_IR_WORKER_LIMIT: usize = 4;
const DEFAULT_PACKAGE_BATCH_SIZE: usize = 512;

/// Number of source packages taken through the main indexing phases together.
///
/// This is a package-level working-set target, not a strict memory limit. A dependency cycle and
/// packages waiting on it may have to stay in one larger batch. Packages retained by the residency
/// policy are not released after their batch finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PackageBatchSize(NonZeroUsize);

impl PackageBatchSize {
    /// Creates a package batch size, returning `None` for zero packages.
    pub fn new(packages: usize) -> Option<Self> {
        NonZeroUsize::new(packages).map(Self)
    }

    /// Returns the configured number of packages.
    pub fn get(self) -> usize {
        self.0.get()
    }
}

impl Default for PackageBatchSize {
    fn default() -> Self {
        Self(
            NonZeroUsize::new(DEFAULT_PACKAGE_BATCH_SIZE)
                .expect("default package batch size should be non-zero"),
        )
    }
}

impl fmt::Display for PackageBatchSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for PackageBatchSize {
    type Err = ParseIntError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse::<NonZeroUsize>().map(Self)
    }
}

/// High-level indexing preference selected by users or frontends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IndexingPerformancePreference {
    /// Prefer lower peak memory when a build phase has to choose a speed/memory trade-off.
    LowerPeakMemory,
    /// Prefer faster indexing when a build phase has to choose a speed/memory trade-off.
    #[default]
    FasterBuilds,
}

impl IndexingPerformancePreference {
    /// Stable kebab-case name accepted by frontends.
    pub fn config_name(self) -> &'static str {
        match self {
            Self::LowerPeakMemory => "lower-peak-memory",
            Self::FasterBuilds => "faster-builds",
        }
    }

    /// Parses the public preference names accepted by frontends.
    pub fn from_config_name(value: &str) -> Option<Self> {
        match value {
            "lower-peak-memory" => Some(Self::LowerPeakMemory),
            "faster-builds" => Some(Self::FasterBuilds),
            _ => None,
        }
    }

    pub(crate) fn macro_expansion_preference(self) -> MacroExpansionPerformancePreference {
        match self {
            Self::LowerPeakMemory => MacroExpansionPerformancePreference::LowerPeakMemory,
            Self::FasterBuilds => MacroExpansionPerformancePreference::FasterBuilds,
        }
    }

    /// Body IR package workers retain sizeable lowering and resolution temporaries concurrently.
    /// Four workers preserve useful package parallelism without multiplying those allocations by
    /// the full machine width.
    pub(crate) fn body_ir_worker_limit(self) -> Option<NonZeroUsize> {
        match self {
            Self::LowerPeakMemory => Some(
                NonZeroUsize::new(LOWER_PEAK_MEMORY_BODY_IR_WORKER_LIMIT)
                    .expect("lower-peak-memory Body IR worker limit should be non-zero"),
            ),
            Self::FasterBuilds => None,
        }
    }
}

/// Controls whether a fresh project build can seed offloadable packages from cache artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartupCacheLoad {
    Disabled,
    #[default]
    Enabled,
}

impl StartupCacheLoad {
    pub(crate) fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

/// Choose whether project construction waits for body analysis before returning.
///
/// The project keeps this choice for source and workspace updates too. With [`Self::EarlyStart`],
/// callers can answer queries about declarations first and build bodies later as needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SplitIndexingMode {
    /// Finish all analysis selected by the build policy before returning the project.
    #[default]
    Full,
    /// Allow returning after collecting declarations, leaving body analysis for later.
    /// Use [`Project::deferred_body_build`](crate::Project::deferred_body_build) for background work
    /// or [`crate::SplitIndexing`] for queries.
    /// Package-batched builds still finish bodies so each batch can be written to disk and released.
    EarlyStart,
}

impl SplitIndexingMode {
    /// Select rebuilt packages whose payloads should be reallocated before return.
    ///
    /// A full build can immediately write and offload packages selected by the residency policy,
    /// so only resident packages need a fresh copy. An ordinary early-start build does not yet
    /// have durable Body IR and must keep every rebuilt package decoded until deferred finishing.
    /// The selection happens before Body IR can prove that an individual package has no deferred
    /// bodies, so the early-start case includes the complete rebuilt set.
    pub(crate) fn packages_to_reallocate(
        self,
        package_residency: &PackageResidencyPlan,
        rebuilt_packages: &[PackageSlot],
    ) -> Vec<PackageSlot> {
        match self {
            Self::Full => package_residency.resident_packages(rebuilt_packages),
            Self::EarlyStart => rebuilt_packages.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use rg_ir_model::PackageSlot;

    use crate::{PackageResidency, PackageResidencyPlan, PackageResidencyPolicy};

    use super::{IndexingPerformancePreference, PackageBatchSize, SplitIndexingMode};

    #[test]
    fn package_batch_size_is_positive() {
        assert!(PackageBatchSize::new(0).is_none());
        assert_eq!(
            PackageBatchSize::new(16)
                .expect("positive package batch size should be accepted")
                .get(),
            16,
        );
        assert_eq!(PackageBatchSize::default().get(), 512);
        assert_eq!(
            "32".parse::<PackageBatchSize>()
                .expect("positive package batch size should parse")
                .get(),
            32,
        );
        assert!("0".parse::<PackageBatchSize>().is_err());
    }

    #[test]
    fn parses_public_preference_names() {
        let preferences = [
            (
                "lower-peak-memory",
                Some(IndexingPerformancePreference::LowerPeakMemory),
            ),
            (
                "faster-builds",
                Some(IndexingPerformancePreference::FasterBuilds),
            ),
            ("lower_peak_memory", None),
            ("unknown", None),
        ];

        for (name, expected) in preferences {
            assert_eq!(
                IndexingPerformancePreference::from_config_name(name),
                expected,
                "{name}",
            );
        }
    }

    #[test]
    fn defaults_to_faster_builds() {
        assert_eq!(
            IndexingPerformancePreference::default(),
            IndexingPerformancePreference::FasterBuilds,
        );
    }

    #[test]
    fn lower_peak_memory_limits_body_ir_to_four_workers() {
        assert_eq!(
            IndexingPerformancePreference::LowerPeakMemory
                .body_ir_worker_limit()
                .map(std::num::NonZeroUsize::get),
            Some(4),
        );
        assert_eq!(
            IndexingPerformancePreference::FasterBuilds.body_ir_worker_limit(),
            None,
        );
    }
    #[test]
    fn early_start_reallocates_packages_waiting_for_deferred_artifacts() {
        let residency = PackageResidencyPlan {
            policy: PackageResidencyPolicy::WorkspaceResident,
            packages: vec![PackageResidency::Offloadable, PackageResidency::Resident],
        };
        let rebuilt = [PackageSlot(0), PackageSlot(1)];

        assert_eq!(
            SplitIndexingMode::Full.packages_to_reallocate(&residency, &rebuilt),
            [PackageSlot(1)],
            "a full build can release its offloadable package immediately",
        );
        assert_eq!(
            SplitIndexingMode::EarlyStart.packages_to_reallocate(&residency, &rebuilt),
            rebuilt,
            "an early-start build retains the offloadable package until Body IR is durable",
        );
    }
}
