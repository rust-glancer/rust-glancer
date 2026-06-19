use ls_types::LSPAny;
use serde::{Deserialize, Serialize};

use super::section;

/// Protocol-level cfg atoms requested by an LSP client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisCfgConfig {
    /// Enables `cfg(test)` for workspace packages during semantic analysis.
    pub test: bool,
    /// Extra `rustc --cfg`-style atoms enabled for every Cargo package.
    pub atoms: Vec<String>,
}

impl AnalysisCfgConfig {
    pub fn from_initialization_options(options: Option<&LSPAny>) -> anyhow::Result<Self> {
        let Some(cfg) = section(options, "cfg") else {
            return Ok(Self::default());
        };

        let default = Self::default();
        let test = match cfg.get("test") {
            Some(value) => value
                .as_bool()
                .ok_or_else(|| anyhow::anyhow!("rust-glancer cfg.test must be a boolean"))?,
            None => default.test,
        };
        let atoms = match cfg.get("atoms") {
            Some(value) => Self::parse_atoms(value)?,
            None => default.atoms,
        };

        Ok(Self { test, atoms })
    }

    fn parse_atoms(value: &LSPAny) -> anyhow::Result<Vec<String>> {
        let atoms = value
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("rust-glancer cfg.atoms must be an array"))?;
        let mut parsed = Vec::new();

        for (idx, atom) in atoms.iter().enumerate() {
            let atom = atom
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("rust-glancer cfg.atoms[{idx}] must be a string"))?
                .trim();
            anyhow::ensure!(
                !atom.is_empty(),
                "rust-glancer cfg.atoms[{idx}] must not be empty",
            );
            anyhow::ensure!(
                is_cfg_atom_name(atom),
                "rust-glancer cfg.atoms[{idx}] must be a cfg atom name such as `tokio_unstable`; key-value cfgs are not supported here",
            );
            if !parsed.iter().any(|known| known == atom) {
                parsed.push(atom.to_string());
            }
        }

        Ok(parsed)
    }
}

fn is_cfg_atom_name(atom: &str) -> bool {
    let mut chars = atom.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|char| char == '_' || char.is_ascii_alphanumeric())
}

impl Default for AnalysisCfgConfig {
    fn default() -> Self {
        Self {
            test: true,
            atoms: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::AnalysisCfgConfig;

    #[test]
    fn parses_cfg_test() {
        let options = json!({
            "cfg": {
                "test": true,
            },
        });

        let config = AnalysisCfgConfig::from_initialization_options(Some(&options))
            .expect("cfg config should parse");

        assert!(config.test);
    }

    #[test]
    fn parses_disabled_cfg_test() {
        let options = json!({
            "cfg": {
                "test": false,
            },
        });

        let config = AnalysisCfgConfig::from_initialization_options(Some(&options))
            .expect("cfg config should parse");

        assert!(!config.test);
    }

    #[test]
    fn parses_custom_cfg_atoms() {
        let options = json!({
            "cfg": {
                "atoms": ["tokio_unstable", "  loom  ", "tokio_unstable"],
            },
        });

        let config = AnalysisCfgConfig::from_initialization_options(Some(&options))
            .expect("cfg config should parse");

        assert_eq!(config.atoms, ["tokio_unstable", "loom"]);
    }

    #[test]
    fn rejects_malformed_cfg_test() {
        let options = json!({
            "cfg": {
                "test": "yes",
            },
        });

        let error = AnalysisCfgConfig::from_initialization_options(Some(&options))
            .expect_err("malformed cfg.test should be rejected");

        assert!(
            error.to_string().contains("rust-glancer cfg.test"),
            "{error:?}",
        );
    }

    #[test]
    fn rejects_malformed_cfg_atoms() {
        let fixtures = [
            (
                json!({ "cfg": { "atoms": true } }),
                "rust-glancer cfg.atoms",
                "non-array cfg atoms should be rejected",
            ),
            (
                json!({ "cfg": { "atoms": [true] } }),
                "rust-glancer cfg.atoms[0]",
                "non-string cfg atoms should be rejected",
            ),
            (
                json!({ "cfg": { "atoms": [""] } }),
                "must not be empty",
                "empty cfg atoms should be rejected",
            ),
            (
                json!({ "cfg": { "atoms": ["feature=\"serde\""] } }),
                "must be a cfg atom name",
                "key-value cfgs should not be accepted as atoms",
            ),
        ];

        for (options, message, err_msg) in fixtures {
            let error =
                AnalysisCfgConfig::from_initialization_options(Some(&options)).expect_err(err_msg);

            assert!(error.to_string().contains(message), "{error:?}");
        }
    }
}
