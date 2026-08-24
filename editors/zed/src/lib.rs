use zed_extension_api::{self as zed, settings::LspSettings};

const SERVER_BINARY: &str = "rust-glancer";
const SERVER_SUBCOMMAND: &str = "lsp";

struct RustGlancerExtension;

impl zed::Extension for RustGlancerExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> zed::Result<zed::Command> {
        let settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree)?;
        let binary = settings.binary;

        // An explicit path makes local development and unusual installations predictable.
        // Otherwise, use the project shell environment that Zed already resolved.
        let command = binary
            .as_ref()
            .and_then(|binary| binary.path.as_deref())
            .filter(|path| !path.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| worktree.which(SERVER_BINARY))
            .ok_or_else(|| {
                "rust-glancer was not found on PATH; configure lsp.rust-glancer.binary.path in Zed settings"
                    .to_string()
            })?;
        let args = binary
            .as_ref()
            .and_then(|binary| binary.arguments.clone())
            .unwrap_or_else(|| vec![SERVER_SUBCOMMAND.to_string()]);
        let env = binary
            .and_then(|binary| binary.env)
            .unwrap_or_default()
            .into_iter()
            .collect();

        Ok(zed::Command { command, args, env })
    }

    fn language_server_initialization_options(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> zed::Result<Option<zed::serde_json::Value>> {
        // Rust Glancer owns its defaults and validation, so the adapter only transports
        // editor settings instead of maintaining a second configuration model.
        Ok(
            LspSettings::for_worktree(language_server_id.as_ref(), worktree)?
                .initialization_options,
        )
    }
}

zed::register_extension!(RustGlancerExtension);
