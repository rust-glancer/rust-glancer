//! Native source actions assembled from current syntax and indexed semantics.
//!
//! The editor text may contain changes that have not been saved, while name resolution and type
//! information come from the last indexed project state. A request is therefore handled in three
//! steps: parse the exact editor text once, let each provider combine it with indexed facts, then
//! validate and order all proposed edits. The result remains in UTF-8 source coordinates; the LSP
//! layer later converts ranges and attaches the editor's document version.

mod import_item;
mod qualified_path;
mod syntax;
mod trait_impl;

use anyhow::Context as _;
use rg_ir_model::CrateRef;
use rg_parse::{FileId, TextSpan};

use crate::{Analysis, CodeAction, CodeActionKind};

use self::syntax::CodeActionSyntax;

/// Which action families the client requested for this query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeActionKinds {
    quick_fix: bool,
    refactor_rewrite: bool,
}

impl CodeActionKinds {
    pub const fn all() -> Self {
        Self {
            quick_fix: true,
            refactor_rewrite: true,
        }
    }

    pub const fn none() -> Self {
        Self {
            quick_fix: false,
            refactor_rewrite: false,
        }
    }

    pub const fn with_quick_fix(mut self, enabled: bool) -> Self {
        self.quick_fix = enabled;
        self
    }

    pub const fn with_refactor_rewrite(mut self, enabled: bool) -> Self {
        self.refactor_rewrite = enabled;
        self
    }

    pub(crate) const fn includes(self, kind: CodeActionKind) -> bool {
        match kind {
            CodeActionKind::QuickFix => self.quick_fix,
            CodeActionKind::RefactorRewrite => self.refactor_rewrite,
        }
    }
}

impl Default for CodeActionKinds {
    fn default() -> Self {
        Self::all()
    }
}

/// Whether the editor asked for actions directly or during automatic lightbulb discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeActionTrigger {
    /// The user explicitly opened or requested the action menu.
    Invoked,
    /// The editor is probing for a passive lightbulb while the user works.
    Automatic,
    /// The client did not provide a recognized trigger kind.
    Unspecified,
}

/// One code-action request in current UTF-8 source coordinates.
#[derive(Debug, Clone, Copy)]
pub struct CodeActionQuery<'source> {
    /// Crate interpretation used for saved semantic facts.
    pub crate_ref: CrateRef,
    /// File inside `crate_ref` that owns the captured source.
    pub file_id: FileId,
    /// Selected UTF-8 range in `source_text`.
    pub range: TextSpan,
    /// Exact editor buffer for syntax selection and source edits.
    pub source_text: &'source str,
    /// Action families that survived the client's request filter.
    pub kinds: CodeActionKinds,
    /// Request origin used to keep expensive discovery out of passive probes.
    pub trigger: CodeActionTrigger,
}

impl<'source> CodeActionQuery<'source> {
    pub fn new(
        crate_ref: CrateRef,
        file_id: FileId,
        range: TextSpan,
        source_text: &'source str,
    ) -> Self {
        Self {
            crate_ref,
            file_id,
            range,
            source_text,
            kinds: CodeActionKinds::all(),
            trigger: CodeActionTrigger::Unspecified,
        }
    }

    pub fn with_kinds(mut self, kinds: CodeActionKinds) -> Self {
        self.kinds = kinds;
        self
    }

    pub fn with_trigger(mut self, trigger: CodeActionTrigger) -> Self {
        self.trigger = trigger;
        self
    }
}

/// Coordinates independent providers over one parse and validates their combined output.
///
/// A provider may decline because its safety proof is incomplete. Provider failures are also kept
/// separate: one broken provider does not hide valid actions produced by another provider.
pub(crate) struct CodeActionResolver<'analysis, 'db, 'source> {
    analysis: &'analysis Analysis<'db>,
    query: CodeActionQuery<'source>,
}

impl<'analysis, 'db, 'source> CodeActionResolver<'analysis, 'db, 'source> {
    pub(crate) fn new(analysis: &'analysis Analysis<'db>, query: CodeActionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Run the requested action providers and combine their answers into one stable list.
    ///
    /// Providers are independent: a trait-member lookup may fail while an import action at the
    /// same position still succeeds. Errors are remembered while usable actions continue through
    /// validation. An error is returned only when no provider produced a valid action.
    pub(crate) fn code_actions(&self) -> anyhow::Result<Vec<CodeAction>> {
        let mut actions = Vec::<CodeAction>::new();
        let mut provider_errors = Vec::new();
        let quick_fix = self.query.kinds.includes(CodeActionKind::QuickFix);
        let rewrite = self.query.kinds.includes(CodeActionKind::RefactorRewrite);
        // 1. Give every provider the same ordinary parse of the captured editor buffer. LSP
        // requests already carry this parse in `CurrentSource`; direct analysis callers fall back
        // to parsing their supplied text here.
        if quick_fix || rewrite {
            let edition = self
                .analysis
                .view_db()
                .crate_edition(self.query.crate_ref)
                .context("read code action crate edition")?;
            let file = self
                .analysis
                .current_source(self.query.crate_ref.package, self.query.file_id)
                .filter(|source| source.text() == self.query.source_text)
                .and_then(|source| source.parse(edition))
                .map_or_else(
                    || rg_parse::parse_source_file(self.query.source_text, edition).tree(),
                    |parse| parse.tree(),
                );
            let syntax = CodeActionSyntax::new(self.query.source_text, file, self.query.range);

            // 2. Run only providers requested by the client. Each provider may return no action or
            // an error without preventing the remaining providers from examining the same syntax.
            if quick_fix {
                match trait_impl::TraitImplCodeActionProvider::new(self.analysis, self.query)
                    .code_action(&syntax)
                {
                    Ok(Some(action)) => actions.push(action),
                    Ok(None) => {}
                    Err(error) => provider_errors.push(error),
                }
                match import_item::ImportItemCodeActionProvider::new(self.analysis, self.query)
                    .code_actions(&syntax)
                {
                    Ok(import_actions) => actions.extend(import_actions),
                    Err(error) => provider_errors.push(error),
                }
            }
            if rewrite {
                match qualified_path::QualifiedPathCodeActionProvider::new(
                    self.analysis,
                    self.query,
                )
                .code_action(&syntax)
                {
                    Ok(Some(action)) => actions.push(action),
                    Ok(None) => {}
                    Err(error) => provider_errors.push(error),
                }
            }
        }

        // 3. Providers own applicability, while the resolver owns the final cross-provider
        // contract:
        // no empty, out-of-bounds, non-UTF-8-boundary, overlapping, or duplicate edit sets.
        actions.retain(|action| {
            self.query.kinds.includes(action.kind)
                && Self::edits_are_valid(self.query.source_text, action)
        });
        actions.sort_by(|left, right| {
            right
                .is_preferred
                .cmp(&left.is_preferred)
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| left.title.cmp(&right.title))
        });
        let mut unique = Vec::new();
        for action in actions {
            if !unique.contains(&action) {
                unique.push(action);
            }
        }
        // One broken provider must not hide valid actions from independent providers. If nobody
        // could answer, keep the first real query error visible instead of turning it into a
        // misleading empty result.
        if unique.is_empty()
            && let Some(error) = provider_errors.into_iter().next()
        {
            return Err(error);
        }
        Ok(unique)
    }

    /// Check that every edit can be applied to this UTF-8 buffer without depending on edit order.
    ///
    /// Besides ordinary overlap, two insertions at the same byte are rejected. Their spans are
    /// both empty, but applying `use A;` and `use B;` in different orders would produce different
    /// source and LSP does not give this model an ordering guarantee.
    fn edits_are_valid(source: &str, action: &CodeAction) -> bool {
        if action.edits.is_empty() {
            return false;
        }
        let source_len = u32::try_from(source.len()).unwrap_or(u32::MAX);
        let mut spans = action
            .edits
            .iter()
            .map(|edit| edit.replace)
            .collect::<Vec<_>>();
        if spans.iter().any(|span| {
            let (Ok(start), Ok(end)) = (
                usize::try_from(span.text.start),
                usize::try_from(span.text.end),
            ) else {
                return true;
            };
            span.text.start > span.text.end
                || span.text.end > source_len
                || !source.is_char_boundary(start)
                || !source.is_char_boundary(end)
        }) {
            return false;
        }
        spans.sort_by_key(|span| (span.text.start, span.text.end));
        spans.windows(2).all(|pair| {
            let [left, right] = pair else {
                return true;
            };
            left.text.end <= right.text.start
                && !(left.is_empty() && right.is_empty() && left.text.start == right.text.start)
        })
    }
}

#[cfg(test)]
mod tests {
    use rg_parse::{Span, TextSpan};

    use crate::{CodeAction, CodeActionEdit, CodeActionKind};

    use super::CodeActionResolver;

    fn action(edits: Vec<CodeActionEdit>) -> CodeAction {
        CodeAction {
            title: "test action".to_string(),
            kind: CodeActionKind::QuickFix,
            is_preferred: false,
            edits,
        }
    }

    fn edit(start: u32, end: u32) -> CodeActionEdit {
        CodeActionEdit {
            replace: Span {
                text: TextSpan { start, end },
            },
            new_text: "replacement".to_string(),
        }
    }

    #[test]
    fn edit_validation_rejects_overlap_and_invalid_utf8_boundaries() {
        let source = "aébc";
        let cases = [
            (vec![edit(0, 1), edit(1, 3)], true, "adjacent edits"),
            (vec![edit(0, 3), edit(1, 4)], false, "overlap"),
            (vec![edit(1, 2)], false, "UTF-8 split"),
            (vec![edit(0, 10)], false, "past end"),
            (vec![edit(1, 1), edit(1, 1)], false, "ambiguous inserts"),
        ];

        for (edits, expected, message) in cases {
            assert_eq!(
                CodeActionResolver::edits_are_valid(source, &action(edits)),
                expected,
                "{message}"
            );
        }
    }
}
