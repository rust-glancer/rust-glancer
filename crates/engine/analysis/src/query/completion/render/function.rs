//! Shared function-completion rendering.
//!
//! Function and method completions need more than a label: they reuse signatures and documentation
//! for display, distinguish receiver methods from associated functions, and turn parameter names
//! into snippet placeholders when the client supports them. Existing call parentheses suppress a
//! second generated argument list.

use rg_ir_view::{
    display::{signature::SignatureRenderer, syntax::SyntaxRenderer},
    member::{FunctionParameterView, MemberFunction},
};

use crate::model::{
    CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem, CompletionKind,
    CompletionTarget,
};

use super::super::CompletionQuery;
use super::{
    CallCompletionKind, escape_lsp_snippet_text,
    sort::{CompletionSortPolicy, CompletionSortPriority},
};

/// Signature metadata and insertion text for one function completion.
struct FunctionCompletionMetadata {
    label: String,
    detail: Option<String>,
    documentation: Option<String>,
    insert_text: CompletionInsertText,
    has_self_receiver: bool,
}

/// Rendered function completion plus receiver information for method-only sites.
pub(crate) struct FunctionCompletion {
    pub(crate) has_self_receiver: bool,
    pub(crate) item: CompletionItem,
}

/// Inputs that vary between function completion sites.
pub(crate) struct FunctionCompletionRequest<'label, 'member> {
    pub(crate) function: MemberFunction<'member>,
    pub(crate) label_override: Option<&'label str>,
    pub(crate) kind: CompletionKind,
    pub(crate) applicability: CompletionApplicability,
    pub(crate) edit: CompletionEdit,
    pub(crate) call_completion: CallCompletionKind,
    pub(crate) sort_policy: CompletionSortPolicy,
    pub(crate) sort_priority: Option<CompletionSortPriority>,
}

/// Applies shared display, documentation, and call-snippet policy to function candidates.
pub(crate) struct FunctionCompletionRenderer<'source> {
    query: CompletionQuery<'source>,
    syntax: SyntaxRenderer,
}

impl<'source> FunctionCompletionRenderer<'source> {
    pub(crate) fn new(query: CompletionQuery<'source>, syntax: SyntaxRenderer) -> Self {
        Self { query, syntax }
    }

    /// Builds display and snippet metadata for a resolved function declaration.
    pub(crate) fn completion(
        &self,
        request: FunctionCompletionRequest<'_, '_>,
    ) -> FunctionCompletion {
        let metadata = self.metadata(
            request.function,
            request.label_override,
            request.call_completion,
            request.edit,
        );
        let target = CompletionTarget::Function(request.function.function_ref());
        let sort_text = request.sort_policy.sort_text(
            request.sort_priority,
            &metadata.label,
            request.kind,
            request.applicability,
            target,
        );

        FunctionCompletion {
            has_self_receiver: metadata.has_self_receiver,
            item: CompletionItem {
                label: metadata.label,
                filter_text: None,
                kind: request.kind,
                target,
                applicability: request.applicability,
                detail: metadata.detail,
                documentation: metadata.documentation,
                sort_text,
                insert_text: metadata.insert_text,
                edit: Some(request.edit),
                additional_edits: Vec::new(),
            },
        }
    }

    fn metadata(
        &self,
        function: MemberFunction<'_>,
        label_override: Option<&str>,
        call_completion: CallCompletionKind,
        edit: CompletionEdit,
    ) -> FunctionCompletionMetadata {
        let label = label_override
            .unwrap_or_else(|| function.name())
            .to_string();
        let label = self.syntax.identifier(&label).to_string();

        FunctionCompletionMetadata {
            label: label.clone(),
            detail: Some(
                SignatureRenderer::new(self.syntax.edition()).member_function_signature(function),
            ),
            documentation: function.docs_text(),
            insert_text: self.insert_text(&label, function, call_completion, edit),
            has_self_receiver: function.has_self_receiver(),
        }
    }

    fn insert_text(
        &self,
        label: &str,
        function: MemberFunction<'_>,
        call_completion: CallCompletionKind,
        edit: CompletionEdit,
    ) -> CompletionInsertText {
        if !call_completion.inserts_call_syntax()
            || !self.query.client_capabilities.snippet_support
            || self.call_parens_already_present(edit)
        {
            return CompletionInsertText::Plain;
        }

        let skip_self = matches!(call_completion, CallCompletionKind::MethodCall);
        CompletionInsertText::Snippet(call_snippet(label, function, skip_self))
    }

    fn call_parens_already_present(&self, edit: CompletionEdit) -> bool {
        let Some(source) = self.query.source_text else {
            return false;
        };
        let Ok(end) = usize::try_from(edit.replace.text.end) else {
            return false;
        };
        let Some(after_completion) = source.get(end..) else {
            return false;
        };

        after_completion.trim_start().starts_with('(')
    }
}

fn call_snippet(label: &str, function: MemberFunction<'_>, skip_self: bool) -> String {
    let mut snippet = escape_lsp_snippet_text(label);
    snippet.push('(');

    for (idx, param) in function
        .parameters()
        .filter(|param| !(skip_self && param.is_receiver()))
        .enumerate()
    {
        if idx > 0 {
            snippet.push_str(", ");
        }
        let placeholder = param_placeholder(param, idx + 1);
        snippet.push_str(&format!(
            "${{{}:{}}}",
            idx + 1,
            escape_lsp_snippet_text(&placeholder)
        ));
    }

    snippet.push(')');
    snippet.push_str("$0");
    snippet
}

fn param_placeholder(param: FunctionParameterView<'_>, idx: usize) -> String {
    let pat = param.pattern().trim();
    simple_binding_name(pat)
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("arg{idx}"))
}

fn simple_binding_name(mut pat: &str) -> Option<&str> {
    loop {
        if let Some(stripped) = pat.strip_prefix("mut ") {
            pat = stripped.trim_start();
        } else if let Some(stripped) = pat.strip_prefix("ref ") {
            pat = stripped.trim_start();
        } else {
            break;
        }
    }

    if pat != "_" && is_ident_like(pat) {
        Some(pat)
    } else {
        None
    }
}

fn is_ident_like(value: &str) -> bool {
    let value = value.strip_prefix("r#").unwrap_or(value);
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }

    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}
