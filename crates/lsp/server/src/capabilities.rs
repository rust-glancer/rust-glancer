use tower_lsp_server::ls_types::*;

pub(crate) fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::UTF16),
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                    include_text: Some(true),
                })),
                ..Default::default()
            },
        )),
        definition_provider: Some(OneOf::Left(true)),
        type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
        implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
        references_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: WorkDoneProgressOptions::default(),
        })),
        document_highlight_provider: Some(OneOf::Left(true)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![
                CodeActionKind::QUICKFIX,
                CodeActionKind::REFACTOR_REWRITE,
            ]),
            resolve_provider: Some(false),
            ..Default::default()
        })),
        completion_provider: Some(CompletionOptions {
            resolve_provider: Some(false),
            trigger_characters: Some(vec![".".to_string(), ":".to_string()]),
            ..Default::default()
        }),
        document_formatting_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        // The VS Code extension sends this request directly, so keep the internal command out of
        // the editor command registry.
        execute_command_provider: None,
        inlay_hint_provider: Some(OneOf::Right(InlayHintServerCapabilities::Options(
            InlayHintOptions {
                resolve_provider: Some(false),
                ..Default::default()
            },
        ))),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        workspace: Some(WorkspaceServerCapabilities {
            workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                supported: Some(true),
                // TODO: Decide if we want to support live workspace-folder updates instead of
                // letting the extension restart the server when the VS Code window shape changes.
                change_notifications: Some(OneOf::Left(false)),
            }),
            file_operations: None,
        }),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use tower_lsp_server::ls_types::{
        CodeActionKind, CodeActionProviderCapability, OneOf, TextDocumentSyncCapability,
        TextDocumentSyncKind,
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

        let Some(CodeActionProviderCapability::Options(code_actions)) =
            capabilities.code_action_provider.as_ref()
        else {
            panic!("code action capability should use explicit options");
        };
        assert_eq!(
            code_actions.code_action_kinds.as_deref(),
            Some(&[CodeActionKind::QUICKFIX, CodeActionKind::REFACTOR_REWRITE,][..])
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

        let Some(OneOf::Right(rename)) = capabilities.rename_provider.as_ref() else {
            panic!("rename capability should use explicit options");
        };
        assert_eq!(rename.prepare_provider, Some(true));

        let Some(TextDocumentSyncCapability::Options(sync)) =
            capabilities.text_document_sync.as_ref()
        else {
            panic!("text document sync should use explicit options");
        };
        assert_eq!(sync.open_close, Some(true));
        assert_eq!(sync.change, Some(TextDocumentSyncKind::INCREMENTAL));
    }

    #[test]
    fn does_not_advertise_internal_reindex_command() {
        assert!(server_capabilities().execute_command_provider.is_none());
    }
}
