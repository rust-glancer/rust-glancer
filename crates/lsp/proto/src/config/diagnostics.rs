use ls_types::LSPAny;
use serde::{Deserialize, Serialize};

use super::{AnalysisConfig, CargoMetadataTarget, section};

/// Cargo diagnostics configuration sent by the LSP client during initialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsConfig {
    pub on_startup: bool,
    pub on_save: bool,
    pub command: String,
    pub cargo_arguments: Vec<String>,
    pub rustc_arguments: Vec<String>,
}

impl DiagnosticsConfig {
    pub fn from_initialization_options(options: Option<&LSPAny>) -> anyhow::Result<Self> {
        let Some(diagnostics) = section(options, "diagnostics") else {
            return Ok(Self::default());
        };

        let on_startup = diagnostics
            .get("onStartup")
            .and_then(LSPAny::as_bool)
            .unwrap_or_default();
        let on_save = diagnostics
            .get("onSave")
            .and_then(LSPAny::as_bool)
            .unwrap_or_default();
        let command = match diagnostics.get("command") {
            Some(command) => {
                let command = command
                    .as_str()
                    .ok_or_else(|| {
                        anyhow::anyhow!("rust-glancer diagnostics.command must be a string")
                    })?
                    .trim();
                validate_cargo_subcommand(command)?;
                command.to_string()
            }
            None => "check".to_string(),
        };
        let cargo_arguments = parse_arguments(diagnostics, "cargoArguments", &["--workspace"])?;
        let rustc_arguments = parse_arguments(diagnostics, "rustcArguments", &[])?;

        Ok(Self {
            on_startup,
            on_save,
            command,
            cargo_arguments,
            rustc_arguments,
        })
    }

    pub fn user_facing_command(&self, analysis: &AnalysisConfig) -> String {
        let mut parts = vec![
            "cargo".to_string(),
            self.command.clone(),
            "--message-format=json".to_string(),
        ];
        parts.extend(self.cargo_arguments(analysis));
        let rustc_arguments = self.rustc_arguments(analysis);
        if !rustc_arguments.is_empty() {
            parts.push("--".to_string());
            parts.extend(rustc_arguments);
        }
        parts.join(" ")
    }

    pub fn cargo_arguments(&self, analysis: &AnalysisConfig) -> Vec<String> {
        let mut arguments = self.cargo_arguments.clone();

        if analysis.cfg.test {
            arguments.push("--all-targets".to_string());
        }
        if let CargoMetadataTarget::Triple(target) = analysis.cargo_metadata_config.target() {
            arguments.push("--target".to_string());
            arguments.push(target.clone());
        }
        if !analysis.cargo_metadata_config.features().is_empty() {
            arguments.push("--features".to_string());
            arguments.push(analysis.cargo_metadata_config.features().join(","));
        }
        if analysis.cargo_metadata_config.all_features_enabled() {
            arguments.push("--all-features".to_string());
        }
        if analysis.cargo_metadata_config.no_default_features_enabled() {
            arguments.push("--no-default-features".to_string());
        }

        arguments
    }

    pub fn rustc_arguments(&self, analysis: &AnalysisConfig) -> Vec<String> {
        let mut arguments = Vec::new();
        for atom in &analysis.cfg.atoms {
            arguments.push("--cfg".to_string());
            arguments.push(atom.clone());
        }
        arguments.extend(self.rustc_arguments.iter().cloned());
        arguments
    }
}

fn validate_cargo_subcommand(command: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !command.is_empty(),
        "rust-glancer diagnostics.command must not be empty",
    );
    anyhow::ensure!(
        !command.starts_with('-'),
        "rust-glancer diagnostics.command must be a Cargo subcommand, not an argument",
    );
    anyhow::ensure!(
        command
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || char == '-' || char == '_'),
        "rust-glancer diagnostics.command must be a single Cargo subcommand such as `check` or `clippy`",
    );

    Ok(())
}

fn parse_arguments(
    diagnostics: &ls_types::LSPObject,
    key: &'static str,
    default: &[&str],
) -> anyhow::Result<Vec<String>> {
    let Some(arguments) = diagnostics.get(key) else {
        return Ok(default
            .iter()
            .map(|argument| argument.to_string())
            .collect());
    };

    arguments
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("rust-glancer diagnostics.{key} must be an array"))?
        .iter()
        .enumerate()
        .map(|(idx, argument)| {
            let argument = argument.as_str().ok_or_else(|| {
                anyhow::anyhow!("rust-glancer diagnostics.{key}[{idx}] must be a string")
            })?;
            validate_diagnostics_argument(key, idx, argument)?;
            Ok(argument.to_string())
        })
        .collect()
}

fn validate_diagnostics_argument(
    key: &'static str,
    idx: usize,
    argument: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !argument.is_empty(),
        "rust-glancer diagnostics.{key}[{idx}] must not be empty",
    );
    anyhow::ensure!(
        !argument.contains('\0'),
        "rust-glancer diagnostics.{key}[{idx}] must not contain NUL bytes",
    );
    anyhow::ensure!(
        argument != "--",
        "rust-glancer diagnostics.{key}[{idx}] must not contain the `--` argument separator; rust-glancer inserts it automatically before diagnostics.rustcArguments",
    );

    Ok(())
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            on_startup: false,
            on_save: false,
            command: "check".to_string(),
            cargo_arguments: vec!["--workspace".to_string()],
            rustc_arguments: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{AnalysisConfig, DiagnosticsConfig};

    #[test]
    fn defaults_to_disabled_cargo_check() {
        let config = DiagnosticsConfig::from_initialization_options(None)
            .expect("default diagnostics config should parse");

        assert!(!config.on_startup);
        assert!(!config.on_save);
        assert_eq!(
            config.user_facing_command(&AnalysisConfig::default()),
            "cargo check --message-format=json --workspace --all-targets"
        );
    }

    #[test]
    fn parses_client_diagnostics_configuration() {
        let options = json!({
            "diagnostics": {
                "onStartup": true,
                "onSave": true,
                "command": "clippy",
                "cargoArguments": ["--workspace", "--locked"],
                "rustcArguments": ["-Dwarnings"],
            },
        });

        let config = DiagnosticsConfig::from_initialization_options(Some(&options))
            .expect("fixture diagnostics config should parse");

        assert!(config.on_startup);
        assert!(config.on_save);
        assert_eq!(config.command, "clippy");
        assert_eq!(config.cargo_arguments, ["--workspace", "--locked"]);
        assert_eq!(config.rustc_arguments, ["-Dwarnings"]);
        assert_eq!(
            config.user_facing_command(&AnalysisConfig::default()),
            "cargo clippy --message-format=json --workspace --locked --all-targets -- -Dwarnings"
        );
    }

    #[test]
    fn user_facing_command_includes_analysis_cargo_settings() {
        let analysis_options = json!({
            "cfg": {
                "test": true,
            },
            "cargo": {
                "target": "x86_64-unknown-linux-gnu",
                "allFeatures": true,
                "noDefaultFeatures": true,
                "features": ["serde", "derive"],
            },
        });
        let analysis = AnalysisConfig::from_initialization_options(Some(&analysis_options))
            .expect("analysis config should parse");
        let config = DiagnosticsConfig::from_initialization_options(None)
            .expect("default diagnostics config should parse");

        assert_eq!(
            config.user_facing_command(&analysis),
            "cargo check --message-format=json --workspace --all-targets --target x86_64-unknown-linux-gnu --features serde,derive --all-features --no-default-features"
        );
    }

    #[test]
    fn disabled_cfg_test_does_not_add_all_targets() {
        let analysis_options = json!({
            "cfg": {
                "test": false,
            },
        });
        let analysis = AnalysisConfig::from_initialization_options(Some(&analysis_options))
            .expect("analysis config should parse");
        let config = DiagnosticsConfig::from_initialization_options(None)
            .expect("default diagnostics config should parse");

        assert_eq!(
            config.user_facing_command(&analysis),
            "cargo check --message-format=json --workspace"
        );
    }

    #[test]
    fn user_facing_command_includes_analysis_cfg_atoms_as_rustc_arguments() {
        let analysis_options = json!({
            "cfg": {
                "atoms": ["tokio_unstable", "loom"],
            },
        });
        let diagnostics_options = json!({
            "diagnostics": {
                "rustcArguments": ["-Dwarnings"],
            },
        });
        let analysis = AnalysisConfig::from_initialization_options(Some(&analysis_options))
            .expect("analysis config should parse");
        let config = DiagnosticsConfig::from_initialization_options(Some(&diagnostics_options))
            .expect("diagnostics config should parse");

        assert_eq!(
            config.user_facing_command(&analysis),
            "cargo check --message-format=json --workspace --all-targets -- --cfg tokio_unstable --cfg loom -Dwarnings"
        );
    }

    #[test]
    fn rejects_empty_diagnostics_command() {
        let options = json!({
            "diagnostics": {
                "command": "  ",
            },
        });

        let error = DiagnosticsConfig::from_initialization_options(Some(&options))
            .expect_err("empty diagnostics command should be rejected");

        assert!(error.to_string().contains("must not be empty"));
    }

    #[test]
    fn rejects_suspicious_diagnostics_command() {
        let options = json!({
            "diagnostics": {
                "command": "check --workspace",
            },
        });

        let error = DiagnosticsConfig::from_initialization_options(Some(&options))
            .expect_err("shell-like diagnostics command should be rejected");

        assert!(error.to_string().contains("single Cargo subcommand"));
    }

    #[test]
    fn rejects_non_string_diagnostics_arguments() {
        let options = json!({
            "diagnostics": {
                "cargoArguments": [true],
            },
        });

        let error = DiagnosticsConfig::from_initialization_options(Some(&options))
            .expect_err("malformed diagnostics argument should be rejected");

        assert!(error.to_string().contains("cargoArguments[0]"));
    }

    #[test]
    fn rejects_argument_separator_in_diagnostics_arguments() {
        let options = json!({
            "diagnostics": {
                "cargoArguments": ["--"],
            },
        });

        let error = DiagnosticsConfig::from_initialization_options(Some(&options))
            .expect_err("argument separator should be rejected");

        assert!(error.to_string().contains("inserts it automatically"));
    }

    #[test]
    fn rejects_argument_separator_in_rustc_diagnostics_arguments() {
        let options = json!({
            "diagnostics": {
                "rustcArguments": ["--"],
            },
        });

        let error = DiagnosticsConfig::from_initialization_options(Some(&options))
            .expect_err("argument separator should be rejected");

        assert!(error.to_string().contains("rustcArguments[0]"));
    }
}
