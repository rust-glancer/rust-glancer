use ls_types::LSPAny;
use serde::{Deserialize, Serialize};

use super::section;

/// Protocol-level standard-library source discovery requested by an LSP client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SysrootDiscovery {
    #[default]
    Auto,
    Disabled,
}

impl SysrootDiscovery {
    pub(super) fn from_initialization_options(options: Option<&LSPAny>) -> anyhow::Result<Self> {
        let Some(value) = section(options, "sysroot").and_then(|sysroot| sysroot.get("discovery"))
        else {
            return Ok(Self::default());
        };

        let value = value
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("rust-glancer sysroot.discovery must be a string"))?;
        Self::from_config_name(value).ok_or_else(|| {
            anyhow::anyhow!("rust-glancer sysroot.discovery must be one of: auto, disabled")
        })
    }

    /// Stable kebab-case name accepted in LSP initialization options.
    pub fn config_name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Disabled => "disabled",
        }
    }

    /// Parses the public names accepted by frontends.
    pub fn from_config_name(value: &str) -> Option<Self> {
        let normalized = value.trim().replace('_', "-").to_ascii_lowercase();
        match normalized.as_str() {
            "auto" => Some(Self::Auto),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::SysrootDiscovery;

    #[test]
    fn parses_sysroot_discovery() {
        let options = json!({
            "sysroot": {
                "discovery": "disabled",
            },
        });

        let config = SysrootDiscovery::from_initialization_options(Some(&options))
            .expect("sysroot config should parse");

        assert_eq!(config, SysrootDiscovery::Disabled);
    }

    #[test]
    fn rejects_unknown_sysroot_discovery() {
        let options = json!({
            "sysroot": {
                "discovery": "manual",
            },
        });

        let error = SysrootDiscovery::from_initialization_options(Some(&options))
            .expect_err("unknown sysroot discovery should be rejected");

        assert!(
            error.to_string().contains("rust-glancer sysroot.discovery"),
            "{error:?}",
        );
    }
}
