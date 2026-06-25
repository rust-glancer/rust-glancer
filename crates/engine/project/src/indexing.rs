//! User-facing indexing trade-offs passed down to build phases.

use rg_def_map::MacroExpansionPerformancePreference;

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
}
