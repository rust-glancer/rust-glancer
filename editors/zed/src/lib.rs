//! Zed adapter for starting Rust Glancer as the Rust language server.
//!
//! This crate owns the small amount of editor-specific policy around the server. It chooses a
//! user-configured binary, a binary from the worktree environment, or the managed fallback. Once
//! the command is chosen, Rust Glancer still owns its command-line and LSP configuration behavior.

use zed_extension_api::{
    self as zed,
    lsp::{Completion, CompletionKind},
    settings::LspSettings,
};

mod server;

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

        // User-controlled installations come first. This keeps local development and system
        // packages predictable; the pinned managed release is only the zero-configuration path.
        let installed_command = binary
            .as_ref()
            .and_then(|binary| binary.path.as_deref())
            .filter(|path| !path.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| worktree.which(server::SERVER_BINARY));
        let command = if let Some(command) = installed_command {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::None,
            );
            command
        } else {
            server::ManagedServer::ensure_installed(language_server_id)?
        };
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

    fn label_for_completion(
        &self,
        _language_server_id: &zed::LanguageServerId,
        completion: Completion,
    ) -> Option<zed::CodeLabel> {
        let details = completion.label_details?;
        let import = details.detail.as_deref()?.strip_prefix(" (use ")?;
        let name_highlight = match completion.kind {
            Some(CompletionKind::Function | CompletionKind::Method) => Some("function"),
            Some(CompletionKind::Constant | CompletionKind::EnumMember) => Some("constant"),
            Some(CompletionKind::Variable) => Some("variable"),
            Some(
                CompletionKind::Class
                | CompletionKind::Interface
                | CompletionKind::Struct
                | CompletionKind::Enum,
            ) => Some("type"),
            _ => None,
        };

        // Zed's generic renderer can drop the import hint when a description is present.
        // Put the import next to the name so it remains visible when the row is truncated.
        let filter_range = (0..completion.label.len()).into();
        let mut spans = vec![
            zed::CodeLabelSpan::literal(completion.label, name_highlight.map(str::to_owned)),
            zed::CodeLabelSpan::literal(" (", None),
            zed::CodeLabelSpan::literal("use", Some("keyword".to_owned())),
            zed::CodeLabelSpan::literal(format!(" {import}"), None),
        ];
        if let Some(declaration) = details.description.or(completion.detail) {
            spans.push(zed::CodeLabelSpan::literal(
                format!("  {declaration}"),
                None,
            ));
        }
        Some(zed::CodeLabel {
            code: String::new(),
            spans,
            filter_range,
        })
    }
}

zed::register_extension!(RustGlancerExtension);
