use ls_types::LSPAny;
use serde::{Deserialize, Serialize};

use super::section;

/// Protocol-level indexing trade-off requested by an LSP client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum IndexingPerformancePreference {
    LowerPeakMemory,
    #[default]
    FasterBuilds,
}

impl IndexingPerformancePreference {
    pub(super) fn from_initialization_options(options: Option<&LSPAny>) -> anyhow::Result<Self> {
        let Some(value) =
            section(options, "indexing").and_then(|indexing| indexing.get("performancePreference"))
        else {
            return Ok(Self::default());
        };

        let value = value.as_str().ok_or_else(|| {
            anyhow::anyhow!("rust-glancer indexing.performancePreference must be a string")
        })?;
        Self::from_config_name(value).ok_or_else(|| {
            anyhow::anyhow!(
                "rust-glancer indexing.performancePreference must be one of: lower-peak-memory, faster-builds"
            )
        })
    }

    /// Stable kebab-case name accepted in LSP initialization options.
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
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::IndexingPerformancePreference;

    #[test]
    fn parses_indexing_preference() {
        let options = json!({
            "indexing": {
                "performancePreference": "faster-builds",
            },
        });

        let config = IndexingPerformancePreference::from_initialization_options(Some(&options))
            .expect("indexing config should parse");

        assert_eq!(config, IndexingPerformancePreference::FasterBuilds);
    }

    #[test]
    fn rejects_unknown_indexing_preference() {
        let options = json!({
            "indexing": {
                "performancePreference": "fast",
            },
        });

        let error = IndexingPerformancePreference::from_initialization_options(Some(&options))
            .expect_err("unknown indexing preference should be rejected");

        assert!(
            error
                .to_string()
                .contains("rust-glancer indexing.performancePreference"),
            "{error:?}",
        );
    }
}
