use ls_types::LSPAny;
use serde::{Deserialize, Serialize};

/// Cargo diagnostics configuration sent by the LSP client during initialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsConfig {
    pub on_startup: bool,
    pub on_save: bool,
    pub command: String,
    pub arguments: Vec<String>,
}

impl DiagnosticsConfig {
    pub fn from_initialization_options(options: Option<&LSPAny>) -> anyhow::Result<Self> {
        let Some(diagnostics) = options
            .and_then(LSPAny::as_object)
            .and_then(|options| options.get("diagnostics"))
            .and_then(LSPAny::as_object)
        else {
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
        let arguments = match diagnostics.get("arguments") {
            Some(arguments) => arguments
                .as_array()
                .ok_or_else(|| {
                    anyhow::anyhow!("rust-glancer diagnostics.arguments must be an array")
                })?
                .iter()
                .enumerate()
                .map(|(idx, argument)| {
                    let argument = argument.as_str().ok_or_else(|| {
                        anyhow::anyhow!(
                            "rust-glancer diagnostics.arguments[{idx}] must be a string"
                        )
                    })?;
                    validate_cargo_argument(idx, argument)?;
                    Ok(argument.to_string())
                })
                .collect::<anyhow::Result<Vec<_>>>()?,
            None => vec!["--workspace".to_string(), "--all-targets".to_string()],
        };

        Ok(Self {
            on_startup,
            on_save,
            command,
            arguments,
        })
    }

    pub fn user_facing_command(&self) -> String {
        let mut parts = vec![
            "cargo".to_string(),
            self.command.clone(),
            "--message-format=json".to_string(),
        ];
        parts.extend(self.arguments.iter().cloned());
        parts.join(" ")
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

fn validate_cargo_argument(idx: usize, argument: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !argument.is_empty(),
        "rust-glancer diagnostics.arguments[{idx}] must not be empty",
    );
    anyhow::ensure!(
        !argument.contains('\0'),
        "rust-glancer diagnostics.arguments[{idx}] must not contain NUL bytes",
    );

    Ok(())
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            on_startup: false,
            on_save: false,
            command: "check".to_string(),
            arguments: vec!["--workspace".to_string(), "--all-targets".to_string()],
        }
    }
}

#[cfg(test)]
mod tests {
    use ls_types::LSPAny;

    use super::DiagnosticsConfig;

    #[test]
    fn defaults_to_disabled_cargo_check() {
        let config = DiagnosticsConfig::from_initialization_options(None)
            .expect("default diagnostics config should parse");

        assert!(!config.on_startup);
        assert!(!config.on_save);
        assert_eq!(
            config.user_facing_command(),
            "cargo check --message-format=json --workspace --all-targets"
        );
    }

    #[test]
    fn parses_client_diagnostics_configuration() {
        let options = object([(
            "diagnostics",
            object([
                ("onStartup", LSPAny::Bool(true)),
                ("onSave", LSPAny::Bool(true)),
                ("command", LSPAny::String("clippy".to_string())),
                (
                    "arguments",
                    LSPAny::Array(vec![
                        LSPAny::String("--workspace".to_string()),
                        LSPAny::String("--all-targets".to_string()),
                        LSPAny::String("--".to_string()),
                        LSPAny::String("-Dwarnings".to_string()),
                    ]),
                ),
            ]),
        )]);

        let config = DiagnosticsConfig::from_initialization_options(Some(&options))
            .expect("fixture diagnostics config should parse");

        assert!(config.on_startup);
        assert!(config.on_save);
        assert_eq!(config.command, "clippy");
        assert_eq!(
            config.arguments,
            ["--workspace", "--all-targets", "--", "-Dwarnings"]
        );
        assert_eq!(
            config.user_facing_command(),
            "cargo clippy --message-format=json --workspace --all-targets -- -Dwarnings"
        );
    }

    #[test]
    fn rejects_empty_diagnostics_command() {
        let options = object([(
            "diagnostics",
            object([("command", LSPAny::String("  ".to_string()))]),
        )]);

        let error = DiagnosticsConfig::from_initialization_options(Some(&options))
            .expect_err("empty diagnostics command should be rejected");

        assert!(error.to_string().contains("must not be empty"));
    }

    #[test]
    fn rejects_suspicious_diagnostics_command() {
        let options = object([(
            "diagnostics",
            object([("command", LSPAny::String("check --workspace".to_string()))]),
        )]);

        let error = DiagnosticsConfig::from_initialization_options(Some(&options))
            .expect_err("shell-like diagnostics command should be rejected");

        assert!(error.to_string().contains("single Cargo subcommand"));
    }

    #[test]
    fn rejects_non_string_diagnostics_arguments() {
        let options = object([(
            "diagnostics",
            object([("arguments", LSPAny::Array(vec![LSPAny::Bool(true)]))]),
        )]);

        let error = DiagnosticsConfig::from_initialization_options(Some(&options))
            .expect_err("malformed diagnostics argument should be rejected");

        assert!(error.to_string().contains("arguments[0]"));
    }

    fn object<const N: usize>(entries: [(&str, LSPAny); N]) -> LSPAny {
        let mut map = match LSPAny::Object(Default::default()) {
            LSPAny::Object(map) => map,
            _ => unreachable!("constructed object should be an object"),
        };
        for (key, value) in entries {
            map.insert(key.to_string(), value);
        }
        LSPAny::Object(map)
    }
}
