//! Synthetic postfix transformations over a semantically indexed dot receiver.
//!
//! Postfix rows do not pretend to be fields or methods. The dot site contributes receiver
//! identity and type inference, request-local syntax contributes the exact text range, and this
//! resolver owns only the transformation templates.
//!
//! ```text
//! (left + right).box$0  -> Box::new(left + right)
//! condition.if$0       -> if condition {
//!                            /* cursor */
//!                        }
//! ```
//!
//! Whole-expression replacement is offered only when the indexed receiver span agrees with the
//! request-local parse. Type-sensitive transforms such as `if`, `while`, and `not` additionally
//! require the receiver to infer as `bool`.

use anyhow::Context as _;
use rg_ir_model::PrimitiveTy;

use crate::{
    Analysis,
    model::{
        CompletionEdit, CompletionInsertText, CompletionItem, CompletionKind,
        SyntheticCompletionTarget,
    },
    query::completion::site::DotCompletionSite,
};

use super::super::{
    CompletionQuery,
    candidates::CompletionCandidateSource,
    render::{SyntheticCompletionCandidate, SyntheticCompletionRenderer, escape_lsp_snippet_text},
    syntax::CompletionSyntaxContext,
};

/// Builds whole-expression edits for synthetic postfix transforms.
///
/// The indexed site supplies receiver identity and inferred type; request syntax supplies the
/// exact source span that can safely be replaced.
pub(super) struct PostfixCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> PostfixCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Complete only when semantic and request-local receiver spans describe the same expression.
    pub(super) fn completions(
        &self,
        site: &DotCompletionSite,
        syntax: Option<&CompletionSyntaxContext<'_>>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let Some(syntax) = syntax else {
            return Ok(Vec::new());
        };
        let Some(receiver_span) = syntax.postfix_receiver_span() else {
            return Ok(Vec::new());
        };

        // Body IR and the request-local parse must select the same written receiver. This check is
        // what makes a whole-expression replacement safe in dirty buffers and parser recovery.
        if receiver_span != site.receiver_span()
            || receiver_span.text.end > site.replace_span().text.start
        {
            return Ok(Vec::new());
        }
        let Some(receiver) = syntax.source_text(receiver_span) else {
            return Ok(Vec::new());
        };
        let replace = rg_parse::Span {
            text: rg_parse::TextSpan {
                start: receiver_span.text.start,
                end: site.replace_span().text.end,
            },
        };
        let between = rg_parse::Span {
            text: rg_parse::TextSpan {
                start: receiver_span.text.end,
                end: site.replace_span().text.start,
            },
        };
        if !syntax
            .source_text(between)
            .is_some_and(|text| text.contains('.'))
        {
            return Ok(Vec::new());
        }

        let prefix = syntax.prefix().text();
        let edit = CompletionEdit { replace };
        let is_bool = CompletionCandidateSource::new(self.analysis.view_db())
            .receiver_primitive_for_dot(site)
            .context("read postfix receiver primitive type")?
            == Some(PrimitiveTy::Bool);
        Ok(SyntheticCompletionRenderer::new(prefix, edit)
            .completions(self.candidates(receiver, is_bool)))
    }

    fn candidates(&self, receiver: &str, is_bool: bool) -> Vec<SyntheticCompletionCandidate> {
        let mut candidates = [
            ("box", "Box::new(expr)", format!("Box::new({receiver})")),
            ("dbg", "dbg!(expr)", format!("dbg!({receiver})")),
            ("err", "Err(expr)", format!("Err({receiver})")),
            ("ok", "Ok(expr)", format!("Ok({receiver})")),
            ("ref", "&expr", format!("&{receiver}")),
            ("refm", "&mut expr", format!("&mut {receiver}")),
            ("some", "Some(expr)", format!("Some({receiver})")),
        ]
        .into_iter()
        .map(|(label, detail, replacement)| {
            Self::candidate(label, detail, CompletionInsertText::Text(replacement))
        })
        .collect::<Vec<_>>();

        let escaped_receiver = escape_lsp_snippet_text(receiver);
        let match_replacement = if self.query.client_capabilities.snippet_support {
            CompletionInsertText::Snippet(format!(
                "match {escaped_receiver} {{\n    ${{1:_}} => {{ $0 }},\n}}"
            ))
        } else {
            CompletionInsertText::Text(format!("match {receiver} {{\n    _ => {{}},\n}}"))
        };
        candidates.push(Self::candidate("match", "match expr", match_replacement));

        // `if`, `while`, and `not` change meaning based on the receiver type. Unknown or non-bool
        // inference omits them instead of producing code that is likely invalid.
        if is_bool {
            let if_replacement = if self.query.client_capabilities.snippet_support {
                CompletionInsertText::Snippet(format!("if {escaped_receiver} {{\n    $0\n}}"))
            } else {
                CompletionInsertText::Text(format!("if {receiver} {{\n}}"))
            };
            candidates.push(Self::candidate("if", "if expr", if_replacement));

            candidates.push(Self::candidate(
                "not",
                "!expr",
                CompletionInsertText::Text(format!("!{receiver}")),
            ));

            let while_replacement = if self.query.client_capabilities.snippet_support {
                CompletionInsertText::Snippet(format!("while {escaped_receiver} {{\n    $0\n}}"))
            } else {
                CompletionInsertText::Text(format!("while {receiver} {{\n}}"))
            };
            candidates.push(Self::candidate("while", "while expr", while_replacement));
        }

        candidates
    }

    fn candidate(
        label: &str,
        detail: &str,
        insert_text: CompletionInsertText,
    ) -> SyntheticCompletionCandidate {
        SyntheticCompletionCandidate::new(
            label,
            CompletionKind::Postfix,
            SyntheticCompletionTarget::Postfix,
        )
        .with_detail(format!("postfix {detail}"))
        .with_insert_text(insert_text)
    }
}
