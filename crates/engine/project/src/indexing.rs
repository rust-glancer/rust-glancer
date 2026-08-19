//! User-facing indexing trade-offs passed down to build phases.

use std::num::NonZeroUsize;

use rg_def_map::MacroExpansionPerformancePreference;

const LOWER_PEAK_MEMORY_BODY_IR_WORKER_LIMIT: usize = 4;

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

#[cfg(test)]
mod tests {
    use super::IndexingPerformancePreference;

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
}
