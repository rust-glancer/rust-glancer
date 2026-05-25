//! Shared function-completion rendering.
//!
//! Function and method completions need more than a label: they reuse signature
//! details for display and turn parameter names into LSP snippet placeholders.

use rg_semantic_ir::ParamItem;

use crate::{
    Analysis,
    api::{render::signature::SignatureRenderer, view::member::MemberFunction},
    model::{
        CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem,
        CompletionKind, CompletionTarget,
    },
};

use super::{
    CompletionQuery,
    completion_sort::{CompletionSortPolicy, CompletionSortPriority},
};

/// Controls whether accepting a function completion inserts a call expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FunctionCallCompletion {
    Plain,
    FunctionCall,
    MethodCall,
}

/// Signature metadata and insertion text for one function completion.
struct FunctionCompletionMetadata {
    label: String,
    detail: Option<String>,
    documentation: Option<String>,
    insert_text: CompletionInsertText,
    has_self_receiver: bool,
}

/// Rendered function completion plus receiver information for method-only sites.
pub(super) struct FunctionCompletion {
    pub(super) has_self_receiver: bool,
    pub(super) item: CompletionItem,
}

/// Inputs that vary between function completion sites.
pub(super) struct FunctionCompletionRequest<'label, 'member> {
    pub(super) function: MemberFunction<'member>,
    pub(super) label_override: Option<&'label str>,
    pub(super) kind: CompletionKind,
    pub(super) applicability: CompletionApplicability,
    pub(super) edit: CompletionEdit,
    pub(super) call_completion: FunctionCallCompletion,
    pub(super) sort_policy: CompletionSortPolicy,
    pub(super) sort_priority: Option<CompletionSortPriority>,
}

pub(super) struct FunctionCompletionRenderer<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> FunctionCompletionRenderer<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Builds display and snippet metadata for a resolved function declaration.
    pub(super) fn completion(
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
                kind: request.kind,
                target,
                applicability: request.applicability,
                detail: metadata.detail,
                documentation: metadata.documentation,
                sort_text,
                insert_text: metadata.insert_text,
                edit: Some(request.edit),
            },
        }
    }

    fn metadata(
        &self,
        function: MemberFunction<'_>,
        label_override: Option<&str>,
        call_completion: FunctionCallCompletion,
        edit: CompletionEdit,
    ) -> FunctionCompletionMetadata {
        let renderer = SignatureRenderer::new(self.analysis);
        let label = label_override
            .unwrap_or_else(|| function.name())
            .to_string();

        FunctionCompletionMetadata {
            label: label.clone(),
            detail: Some(renderer.member_function_signature(&function)),
            documentation: function.docs_text(),
            insert_text: self.insert_text(&label, function.params(), call_completion, edit),
            has_self_receiver: function.has_self_receiver(),
        }
    }

    fn insert_text(
        &self,
        label: &str,
        params: &[ParamItem],
        call_completion: FunctionCallCompletion,
        edit: CompletionEdit,
    ) -> CompletionInsertText {
        if matches!(call_completion, FunctionCallCompletion::Plain)
            || !self.query.client_capabilities.snippet_support
            || self.call_parens_already_present(edit)
        {
            return CompletionInsertText::Plain;
        }

        let skip_self = matches!(call_completion, FunctionCallCompletion::MethodCall);
        CompletionInsertText::Snippet(call_snippet(label, params, skip_self))
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

fn call_snippet(label: &str, params: &[ParamItem], skip_self: bool) -> String {
    let mut snippet = snippet_text(label);
    snippet.push('(');

    for (idx, param) in params
        .iter()
        .enumerate()
        .filter(|(param_idx, _)| !(skip_self && *param_idx == 0))
        .map(|(_, param)| param)
        .enumerate()
    {
        if idx > 0 {
            snippet.push_str(", ");
        }
        let placeholder = param_placeholder(param, idx + 1);
        snippet.push_str(&format!("${{{}:{}}}", idx + 1, snippet_text(&placeholder)));
    }

    snippet.push(')');
    snippet.push_str("$0");
    snippet
}

fn param_placeholder(param: &ParamItem, idx: usize) -> String {
    let pat = param.pat.trim();
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

fn snippet_text(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        if matches!(ch, '\\' | '$' | '}') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}
