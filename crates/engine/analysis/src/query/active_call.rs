//! Finds the call and argument slot shared by signature help and completion.
//!
//! In `send(value, $0)`, syntax tells us which slot the user is filling. Body IR supplies
//! the callee and which of the written arguments are enabled by cfg.

use rg_ir_model::{CrateRef, FileId, Span};
use rg_ir_view::ty::locals::{BodyView, CallTarget, ResolvedCall};
use rg_syntax::{AstNode as _, SyntaxKind, TextSize, ast};

use crate::Analysis;

#[derive(Clone, Copy)]
pub(super) enum ActiveArgument {
    Empty,
    /// The written span lets completion check which lowered expression could have supplied a type.
    Written(Span),
    Disabled,
}

/// The slot index counts arguments inside the parentheses. For `client.send($0)`, it
/// is zero. A consumer of the full method signature adds the offset for `self`.
pub(super) struct ActiveCall {
    call: ResolvedCall<CallTarget>,
    span: Span,
    argument_index: usize,
    argument: ActiveArgument,
}

impl ActiveCall {
    pub(super) fn at(
        analysis: &Analysis<'_>,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<Self>> {
        let source = analysis.source_syntax(crate_ref, file_id)?;
        let offset = TextSize::from(offset);
        if offset > source.syntax().text_range().end() {
            return Ok(None);
        }

        // At `send($0)` or `send(value,$0)`, start with the opening paren or comma on the left.
        // An unfinished list can end before the cursor. In `send(value, $0`, walk back over the
        // space to find the comma inside the list.
        let Some(mut token) = source.syntax().token_at_offset(offset).left_biased() else {
            return Ok(None);
        };
        while token.kind().is_trivia() {
            let Some(previous) = token.prev_token() else {
                return Ok(None);
            };
            token = previous;
        }
        // Check the innermost list first. In `outer(inner()$0, value)`, the cursor is past the
        // inner closing paren, so keep walking up to `outer`.
        let arguments = token
            .parent_ancestors()
            .filter_map(ast::ArgList::cast)
            .find(|list| {
                list.l_paren_token()
                    .is_some_and(|open| open.text_range().end() <= offset)
                    && list
                        .r_paren_token()
                        .is_none_or(|close| offset <= close.text_range().start())
            });
        let Some(arguments) = arguments else {
            return Ok(None);
        };
        let Some(call_syntax) = arguments.syntax().parent() else {
            return Ok(None);
        };

        let db = analysis.view_db();
        let call_span = Span::from_text_range(call_syntax.text_range());
        let Some(call) = BodyView::new(db).resolved_call(crate_ref, file_id, call_span)? else {
            return Ok(None);
        };
        // Count the commas in this list before the cursor. The comma in `(1, 2)` belongs to the
        // tuple. The parser wraps the comma in `send(, $0)` in an error node. That comma still
        // ends the first slot.
        //
        // Body IR skips cfg-disabled arguments. In `send(#[cfg(any())] (), $0value)`, the next
        // lowered span belongs to `value`. Leave it in the iterator and skip the comma after
        // `()` so `value` still gets the first parameter.
        let mut lowered = call.args().iter().peekable();
        let mut disabled_argument = false;
        let mut argument_index = 0;
        let mut argument = ActiveArgument::Empty;
        for child in arguments
            .syntax()
            .children_with_tokens()
            .take_while(|child| child.text_range().start() <= offset)
        {
            let separator = child.kind() == SyntaxKind::COMMA
                || (child.kind() == SyntaxKind::ERROR
                    && child
                        .as_node()
                        .and_then(|error| error.first_child_or_token())
                        .is_some_and(|token| token.kind() == SyntaxKind::COMMA));
            if separator && child.text_range().end() <= offset {
                argument_index += usize::from(!disabled_argument);
                disabled_argument = false;
            } else if let Some(expr) = child.into_node().and_then(ast::Expr::cast) {
                let span = Span::from_text_range(expr.syntax().text_range());
                // A macro-expanded argument can point to a token within its written expression.
                disabled_argument = lowered
                    .next_if(|arg| span.contains_span(arg.span()))
                    .is_none();
                if span.end >= u32::from(offset) {
                    argument = if disabled_argument {
                        ActiveArgument::Disabled
                    } else {
                        ActiveArgument::Written(span)
                    };
                    break;
                }
            }
        }

        Ok(Some(Self {
            call,
            span: call_span,
            argument_index,
            argument,
        }))
    }

    pub(super) fn call(&self) -> &ResolvedCall<CallTarget> {
        &self.call
    }

    pub(super) fn argument_index(&self) -> usize {
        self.argument_index
    }

    pub(super) fn argument(&self) -> ActiveArgument {
        self.argument
    }

    pub(super) fn span(&self) -> Span {
        self.span
    }
}
