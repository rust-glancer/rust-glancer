use std::num::NonZeroUsize;

use ls_types::LSPAny;
use serde::{Deserialize, Serialize};

use super::section;

const DEFAULT_PACKAGE_BATCH_SIZE: usize = 512;

/// Number of source packages processed together by lower-memory batch indexing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PackageBatchSize(NonZeroUsize);

impl PackageBatchSize {
    pub(super) fn from_initialization_options(options: Option<&LSPAny>) -> anyhow::Result<Self> {
        let Some(value) =
            section(options, "indexing").and_then(|indexing| indexing.get("packageBatchSize"))
        else {
            return Ok(Self::default());
        };
        let Some(value) = value.as_u64() else {
            anyhow::bail!("rust-glancer indexing.packageBatchSize must be a positive integer");
        };
        let Ok(value) = usize::try_from(value) else {
            anyhow::bail!(
                "rust-glancer indexing.packageBatchSize must fit in this platform's package count"
            );
        };
        let Some(value) = NonZeroUsize::new(value) else {
            anyhow::bail!("rust-glancer indexing.packageBatchSize must be a positive integer");
        };

        Ok(Self(value))
    }

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

    use super::{IndexingPerformancePreference, PackageBatchSize};

    #[test]
    fn parses_package_batch_size() {
        let options = json!({
            "indexing": {
                "packageBatchSize": 16,
            },
        });

        let size = PackageBatchSize::from_initialization_options(Some(&options))
            .expect("positive package batch size should parse");

        assert_eq!(size.get(), 16);
    }

    #[test]
    fn rejects_invalid_package_batch_sizes() {
        for value in [json!(0), json!(-1), json!(1.5), json!("16")] {
            let options = json!({
                "indexing": {
                    "packageBatchSize": value,
                },
            });

            let error = PackageBatchSize::from_initialization_options(Some(&options))
                .expect_err("invalid package batch size should be rejected");

            assert!(
                error
                    .to_string()
                    .contains("rust-glancer indexing.packageBatchSize"),
                "{error:?}",
            );
        }
    }

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
