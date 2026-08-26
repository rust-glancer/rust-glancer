//! Code-action request facts shared across the server/engine RPC boundary.

use serde::{Deserialize, Serialize};

/// Client features that control publication of action literals with inline edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct CodeActionClientCapabilities {
    /// The client accepts full `CodeAction` literals instead of commands alone.
    pub literal_support: bool,
    /// Workspace edits may use versioned `documentChanges`.
    pub versioned_document_edits: bool,
    /// The client understands the optional `isPreferred` marker.
    pub preferred_support: bool,
}

impl CodeActionClientCapabilities {
    /// Read the features that gate eager edits or optional action metadata.
    pub fn from_lsp_client_capabilities(capabilities: &ls_types::ClientCapabilities) -> Self {
        let code_action = capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.code_action.as_ref());
        let workspace_edit = capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.workspace_edit.as_ref());
        Self {
            literal_support: code_action
                .and_then(|capability| capability.code_action_literal_support.as_ref())
                .is_some(),
            versioned_document_edits: workspace_edit
                .and_then(|capability| capability.document_changes)
                .unwrap_or(false),
            preferred_support: code_action
                .and_then(|capability| capability.is_preferred_support)
                .unwrap_or(false),
        }
    }

    /// Return whether a full action with versioned edits can be sent without a resolve request.
    pub const fn supports_eager_actions(self) -> bool {
        self.literal_support && self.versioned_document_edits
    }
}

/// Action families that survive the client's `CodeActionContext.only` filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct CodeActionRequestKinds {
    quick_fix: bool,
    refactor_rewrite: bool,
}

impl CodeActionRequestKinds {
    /// Translate LSP's hierarchical `only` filter into the action families implemented here.
    pub fn from_lsp(only: Option<&[ls_types::CodeActionKind]>) -> Self {
        let Some(only) = only else {
            return Self {
                quick_fix: true,
                refactor_rewrite: true,
            };
        };
        Self {
            quick_fix: Self::requested(only, ls_types::CodeActionKind::QUICKFIX.as_str()),
            refactor_rewrite: Self::requested(
                only,
                ls_types::CodeActionKind::REFACTOR_REWRITE.as_str(),
            ),
        }
    }

    /// Check a hierarchical action kind such as `refactor.rewrite` against requested parents.
    ///
    /// Requesting `refactor` includes `refactor.rewrite`, while requesting `source` does not.
    fn requested(only: &[ls_types::CodeActionKind], candidate: &str) -> bool {
        only.iter().any(|requested| {
            let requested = requested.as_str();
            requested.is_empty()
                || candidate == requested
                || candidate
                    .strip_prefix(requested)
                    .is_some_and(|suffix| suffix.starts_with('.'))
        })
    }

    pub const fn quick_fix(self) -> bool {
        self.quick_fix
    }

    pub const fn refactor_rewrite(self) -> bool {
        self.refactor_rewrite
    }
}

/// Request origin used to keep expensive providers out of automatic lightbulb probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum CodeActionRequestTrigger {
    /// The user explicitly requested actions.
    Invoked,
    /// The editor requested passive lightbulb discovery.
    Automatic,
    /// The client omitted or supplied an unknown trigger kind.
    Unspecified,
}

/// The two request facts analysis needs before deciding which providers may run.
///
/// `only` selects action families, while the trigger says whether the user explicitly asked or the
/// editor is only probing for a lightbulb. Diagnostics are not copied because these providers
/// derive applicability from source and indexed semantics instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct CodeActionRequestContext {
    kinds: CodeActionRequestKinds,
    trigger: CodeActionRequestTrigger,
}

impl CodeActionRequestContext {
    /// Keep the kind filter and trigger, which are the only context fields used by providers.
    pub fn from_lsp(context: &ls_types::CodeActionContext) -> Self {
        let trigger = match context.trigger_kind {
            Some(trigger) if trigger == ls_types::CodeActionTriggerKind::INVOKED => {
                CodeActionRequestTrigger::Invoked
            }
            Some(trigger) if trigger == ls_types::CodeActionTriggerKind::AUTOMATIC => {
                CodeActionRequestTrigger::Automatic
            }
            Some(_) | None => CodeActionRequestTrigger::Unspecified,
        };
        Self {
            kinds: CodeActionRequestKinds::from_lsp(context.only.as_deref()),
            trigger,
        }
    }

    pub const fn kinds(self) -> CodeActionRequestKinds {
        self.kinds
    }

    pub const fn trigger(self) -> CodeActionRequestTrigger {
        self.trigger
    }
}

#[cfg(test)]
mod tests {
    use ls_types::{ClientCapabilities, CodeActionContext, CodeActionKind, CodeActionTriggerKind};

    use super::{
        CodeActionClientCapabilities, CodeActionRequestContext, CodeActionRequestKinds,
        CodeActionRequestTrigger,
    };

    #[test]
    fn reads_every_capability_required_by_eager_actions() {
        let capabilities: ClientCapabilities = serde_json::from_value(serde_json::json!({
            "workspace": {
                "workspaceEdit": { "documentChanges": true }
            },
            "textDocument": {
                "codeAction": {
                    "codeActionLiteralSupport": {
                        "codeActionKind": { "valueSet": ["quickfix", "refactor.rewrite"] }
                    },
                    "isPreferredSupport": true
                }
            }
        }))
        .expect("code action client capabilities should deserialize");

        let actual = CodeActionClientCapabilities::from_lsp_client_capabilities(&capabilities);

        assert!(actual.literal_support);
        assert!(actual.versioned_document_edits);
        assert!(actual.preferred_support);
        assert!(actual.supports_eager_actions());

        let missing_versioned_edits = ClientCapabilities::default();
        assert!(
            !CodeActionClientCapabilities::from_lsp_client_capabilities(&missing_versioned_edits,)
                .supports_eager_actions()
        );
    }

    #[test]
    fn parent_kinds_include_supported_descendants() {
        let refactor = [CodeActionKind::REFACTOR];
        let kinds = CodeActionRequestKinds::from_lsp(Some(&refactor));
        assert!(!kinds.quick_fix());
        assert!(kinds.refactor_rewrite());

        let quick_fix = [CodeActionKind::QUICKFIX];
        let kinds = CodeActionRequestKinds::from_lsp(Some(&quick_fix));
        assert!(kinds.quick_fix());
        assert!(!kinds.refactor_rewrite());
    }

    #[test]
    fn request_context_preserves_trigger_and_unsupported_kind_filters() {
        let context = CodeActionContext {
            diagnostics: Vec::new(),
            only: Some(vec![CodeActionKind::SOURCE]),
            trigger_kind: Some(CodeActionTriggerKind::AUTOMATIC),
        };

        let context = CodeActionRequestContext::from_lsp(&context);

        assert_eq!(context.trigger(), CodeActionRequestTrigger::Automatic);
        assert!(!context.kinds().quick_fix());
        assert!(!context.kinds().refactor_rewrite());
    }
}
