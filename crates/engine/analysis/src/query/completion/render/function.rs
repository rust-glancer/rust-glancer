//! Shared function-completion rendering.
//!
//! Function and method completions need more than a label: they reuse signatures and documentation
//! for display, distinguish receiver methods from associated functions, and turn parameter names
//! into snippet placeholders when the client supports them. Existing call parentheses suppress a
//! second generated argument list.

use rg_ir_view::{
    IndexedViewDb,
    display::{signature::SignatureRenderer, syntax::SyntaxRenderer},
    member::MemberFunction,
    ty::TyView,
};

use super::{CallCompletionKind, escape_lsp_snippet_text};
use crate::{
    model::{
        CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem,
        CompletionKind, CompletionTarget,
    },
    query::completion::{
        CompletionContext,
        relevance::{CompletionSortPolicy, CompletionSortPriority},
    },
};

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
pub(crate) struct FunctionCompletionRenderer<'a, 'db, 'source> {
    db: &'a IndexedViewDb<'db>,
    query: CompletionContext<'source>,
    syntax: SyntaxRenderer,
}

impl<'a, 'db, 'source> FunctionCompletionRenderer<'a, 'db, 'source> {
    pub(crate) fn new(
        db: &'a IndexedViewDb<'db>,
        query: CompletionContext<'source>,
        syntax: SyntaxRenderer,
    ) -> Self {
        Self { db, query, syntax }
    }

    /// Builds display and snippet metadata for a resolved function declaration.
    pub(crate) fn completion(
        &self,
        request: FunctionCompletionRequest<'_, '_>,
    ) -> anyhow::Result<FunctionCompletion> {
        let function = request.function;
        let label = request.label_override.unwrap_or_else(|| function.name());
        let label = self.syntax.identifier(label).to_string();
        let inserts_call = request.call_completion.inserts_call_syntax()
            && self.query.client_capabilities.snippet_support
            && !self.call_parens_already_present(request.edit);
        let insert_text = if inserts_call {
            let skip_self = matches!(request.call_completion, CallCompletionKind::MethodCall);
            CompletionInsertText::Snippet(Self::call_snippet(&label, function, skip_self))
        } else {
            CompletionInsertText::Plain
        };

        let target = CompletionTarget::Function(function.function_ref());
        let mut item = CompletionItem {
            relevance: request.sort_policy.relevance(request.sort_priority),
            label,
            filter_text: None,
            kind: request.kind,
            target,
            applicability: request.applicability,
            detail: Some(
                SignatureRenderer::new(self.syntax.edition()).member_function_signature(function),
            ),
            documentation: function.docs_text(),
            insert_text,
            edit: Some(request.edit),
            import: None,
        };
        // Dot auto-import candidates already have a proven receiver match. A matching return
        // type can make an extension method worth its import.
        if let Some(expected) = self.query.expected_type
            && (matches!(request.call_completion, CallCompletionKind::MethodCall)
                || !matches!(
                    request.sort_priority,
                    Some(CompletionSortPriority::AutoImport { .. })
                ))
        {
            expected.rank_function(
                TyView::new(self.db),
                function.function_ref(),
                inserts_call,
                &mut item,
            )?;
        }
        Ok(FunctionCompletion {
            has_self_receiver: function.has_self_receiver(),
            item,
        })
    }

    fn call_parens_already_present(&self, edit: CompletionEdit) -> bool {
        if self
            .query
            .expected_type
            .is_some_and(|expected| expected.is_call())
        {
            return true;
        }
        let Ok(end) = usize::try_from(edit.replace.end) else {
            return false;
        };
        let Some(after_completion) = self.query.source_text.get(end..) else {
            return false;
        };

        after_completion.trim_start().starts_with('(')
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
            let placeholder = param
                .simple_binding_name()
                .map_or_else(|| format!("arg{}", idx + 1), str::to_owned);
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
}
