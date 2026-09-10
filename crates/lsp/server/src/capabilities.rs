use tower_lsp_server::gen_lsp_types::*;

pub(crate) fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::UTF16),
        text_document_sync: Some(TextDocumentSync::Options(TextDocumentSyncOptions {
            open_close: Some(true),
            change: Some(TextDocumentSyncKind::Incremental),
            save: Some(Save::SaveOptions(SaveOptions {
                include_text: Some(true),
            })),
            ..Default::default()
        })),
        definition_provider: Some(true.into()),
        type_definition_provider: Some(true.into()),
        implementation_provider: Some(true.into()),
        references_provider: Some(true.into()),
        rename_provider: Some(RenameProvider::RenameOptions(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: WorkDoneProgressOptions::default(),
        })),
        document_highlight_provider: Some(true.into()),
        hover_provider: Some(true.into()),
        code_action_provider: Some(CodeActionProvider::CodeActionOptions(CodeActionOptions {
            code_action_kinds: Some(vec![
                CodeActionKind::QuickFix,
                CodeActionKind::RefactorRewrite,
            ]),
            resolve_provider: Some(false),
            ..Default::default()
        })),
        completion_provider: Some(CompletionOptions {
            resolve_provider: Some(false),
            trigger_characters: Some(vec![".".to_string(), ":".to_string()]),
            ..Default::default()
        }),
        document_formatting_provider: Some(true.into()),
        document_symbol_provider: Some(true.into()),
        folding_range_provider: Some(true.into()),
        semantic_tokens_provider: Some(SemanticTokensProvider::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: rg_lsp_proto::semantic_tokens_legend(),
                full: Some(Full::Bool(true)),
                range: Some(true.into()),
                ..Default::default()
            },
        )),
        // The VS Code extension sends this request directly, so keep the internal command out of
        // the editor command registry.
        execute_command_provider: None,
        inlay_hint_provider: Some(InlayHintProvider::InlayHintOptions(InlayHintOptions {
            resolve_provider: Some(false),
            ..Default::default()
        })),
        workspace_symbol_provider: Some(true.into()),
        workspace: Some(WorkspaceOptions {
            workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                supported: Some(true),
                // TODO: Decide if we want to support live workspace-folder updates instead of
                // letting the extension restart the server when the VS Code window shape changes.
                change_notifications: Some(false.into()),
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use tower_lsp_server::gen_lsp_types::{
        CodeActionKind, CodeActionProvider, Full, RenameProvider, SemanticTokensProvider,
        TextDocumentSync, TextDocumentSyncKind,
    };

    use super::server_capabilities;

    #[test]
    fn advertises_the_supported_lsp_surface() {
        let capabilities = server_capabilities();

        let workspace_folders = capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.workspace_folders.as_ref())
            .expect("workspace folder capability should stay explicit");
        assert_eq!(workspace_folders.supported, Some(true));

        assert!(capabilities.inlay_hint_provider.is_some());
        assert!(capabilities.hover_provider.is_some());
        assert!(capabilities.implementation_provider.is_some());
        assert!(capabilities.references_provider.is_some());
        assert!(capabilities.document_formatting_provider.is_some());
        assert!(capabilities.document_highlight_provider.is_some());
        assert!(capabilities.folding_range_provider.is_some());
        let Some(SemanticTokensProvider::SemanticTokensOptions(tokens)) =
            capabilities.semantic_tokens_provider
        else {
            panic!("semantic tokens should advertise the engine legend");
        };
        assert_eq!(tokens.legend, rg_lsp_proto::semantic_tokens_legend());
        assert_eq!(tokens.full, Some(Full::Bool(true)));
        assert_eq!(tokens.range, Some(true.into()));

        let Some(CodeActionProvider::CodeActionOptions(code_actions)) =
            capabilities.code_action_provider.as_ref()
        else {
            panic!("code action capability should use explicit options");
        };
        assert_eq!(
            code_actions.code_action_kinds.as_deref(),
            Some(&[CodeActionKind::QuickFix, CodeActionKind::RefactorRewrite,][..])
        );
        assert_eq!(code_actions.resolve_provider, Some(false));

        let completion = capabilities
            .completion_provider
            .as_ref()
            .expect("completion capability should stay explicit");
        assert_eq!(
            completion.trigger_characters.as_deref(),
            Some(&[".".to_string(), ":".to_string()][..])
        );

        let Some(RenameProvider::RenameOptions(rename)) = capabilities.rename_provider.as_ref()
        else {
            panic!("rename capability should use explicit options");
        };
        assert_eq!(rename.prepare_provider, Some(true));

        let Some(TextDocumentSync::Options(sync)) = capabilities.text_document_sync.as_ref() else {
            panic!("text document sync should use explicit options");
        };
        assert_eq!(sync.open_close, Some(true));
        assert_eq!(sync.change, Some(TextDocumentSyncKind::Incremental));
    }

    #[test]
    fn does_not_advertise_internal_reindex_command() {
        assert!(server_capabilities().execute_command_provider.is_none());
    }
}
