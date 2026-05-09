use tower_lsp_server::ls_types::*;

pub(crate) fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::UTF16),
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::FULL),
                save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                    include_text: Some(true),
                })),
                ..Default::default()
            },
        )),
        definition_provider: Some(OneOf::Left(true)),
        type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        completion_provider: Some(CompletionOptions {
            resolve_provider: Some(false),
            trigger_characters: Some(vec![".".to_string()]),
            ..Default::default()
        }),
        document_symbol_provider: Some(OneOf::Left(true)),
        // The VS Code extension sends this request directly. Advertising the internal command
        // makes vscode-languageclient register it as a global VS Code command, which collides when
        // the extension runs one language client per workspace folder.
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
                supported: Some(false), // TODO: We might in fact want to support it eventually (low prio though)
                change_notifications: Some(OneOf::Left(false)),
            }),
            file_operations: None,
        }),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::server_capabilities;
    use tower_lsp_server::ls_types::{TextDocumentSyncCapability, TextDocumentSyncKind};

    #[test]
    fn does_not_advertise_multi_root_workspace_support_yet() {
        let capabilities = server_capabilities();
        let workspace_folders = capabilities
            .workspace
            .and_then(|workspace| workspace.workspace_folders)
            .expect("workspace folder capability should stay explicit");

        assert_eq!(workspace_folders.supported, Some(false));
    }

    #[test]
    fn advertises_static_inlay_hint_support() {
        let capabilities = server_capabilities();
        assert!(capabilities.inlay_hint_provider.is_some());
    }

    #[test]
    fn advertises_hover_support() {
        let capabilities = server_capabilities();
        assert!(capabilities.hover_provider.is_some());
    }

    #[test]
    fn does_not_advertise_internal_reindex_command() {
        let capabilities = server_capabilities();

        assert!(capabilities.execute_command_provider.is_none());
    }

    #[test]
    fn tracks_open_and_changed_documents_without_unsaved_analysis() {
        let capabilities = server_capabilities();
        let Some(TextDocumentSyncCapability::Options(sync)) = capabilities.text_document_sync
        else {
            panic!("text document sync should use explicit options");
        };

        assert_eq!(sync.open_close, Some(true));
        assert_eq!(sync.change, Some(TextDocumentSyncKind::FULL));
    }
}
