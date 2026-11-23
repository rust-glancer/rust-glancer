//! Signature help for calls in the editor's source.
//!
//! Syntax tells us where the cursor falls even when the next argument is still missing.
//! Body IR supplies the target declaration or callable type and the arguments enabled by cfg.

use anyhow::Context as _;
use rg_ir_model::{CrateRef, FileId, Span};
use rg_ir_view::{
    display::signature::{CallSignature, SignatureRenderer},
    member::MemberView,
    ty::locals::{BodyView, CallTarget, ResolvedCall},
};
use rg_parse::syntax_edition;
use rg_std::ExpectedUnique;
use rg_syntax::{AstNode as _, SyntaxKind, TextRange, TextSize, ast};

use super::copied_call;
use crate::{Analysis, SignatureHelp};

pub(crate) fn signature_help(
    analysis: &Analysis<'_>,
    crate_ref: CrateRef,
    file_id: FileId,
    offset: u32,
) -> anyhow::Result<Option<SignatureHelp>> {
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
    let db = analysis.view_db();
    let bodies = BodyView::new(db);

    if token
        .parent_ancestors()
        .any(|node| ast::TokenTree::can_cast(node.kind()))
    {
        let mut calls = bodies.copied_calls_at(crate_ref, file_id, u32::from(offset))?;
        calls.sort_by_key(|call| (call.source().span.len(), call.source().span.start));
        let text = source.syntax().text();
        let edition = syntax_edition(db.crate_edition(crate_ref)?);
        let mut selected_len = None;
        let mut help = ExpectedUnique::new();
        for copies in calls.chunk_by(|left, right| left.source().span == right.source().span) {
            let span = copies[0].source().span;
            if selected_len.is_some_and(|len| len < span.len()) {
                break;
            }
            let fragment = text
                .slice(TextRange::new(span.start.into(), span.end.into()))
                .to_string();
            let local_offset = offset - TextSize::from(span.start);
            let Some(arguments) = copied_call::argument_list(&fragment, edition)
                .filter(|list| contains_offset(list, local_offset))
            else {
                continue;
            };
            selected_len = Some(span.len());
            for call in copies {
                // An unresolved or differently resolved copy cannot pick a signature for
                // the other copies at the same written site.
                let Some(target) = call.target() else {
                    return Ok(None);
                };
                help.push(render(
                    analysis,
                    crate_ref,
                    call,
                    target,
                    &arguments,
                    local_offset,
                    span.start,
                )?);
            }
        }
        if selected_len.is_some() {
            return Ok(help.into_option().flatten());
        }
    }

    // Check the innermost list first. In `outer(inner()$0, value)`, the cursor is past the
    // inner closing paren, so keep walking up to `outer`.
    let arguments = token
        .parent_ancestors()
        .filter_map(ast::ArgList::cast)
        .find(|list| contains_offset(list, offset));
    let Some(arguments) = arguments else {
        return Ok(None);
    };
    let Some(call_syntax) = arguments.syntax().parent() else {
        return Ok(None);
    };

    let call_span = Span::from_text_range(call_syntax.text_range());
    let Some(call) = bodies.resolved_call(crate_ref, file_id, call_span)? else {
        return Ok(None);
    };
    render(
        analysis,
        crate_ref,
        &call,
        call.target(),
        &arguments,
        offset,
        0,
    )
}

fn contains_offset(arguments: &ast::ArgList, offset: TextSize) -> bool {
    arguments
        .l_paren_token()
        .is_some_and(|open| open.text_range().end() <= offset)
        && arguments
            .r_paren_token()
            .is_none_or(|close| offset <= close.text_range().start())
}

fn render<T>(
    analysis: &Analysis<'_>,
    crate_ref: CrateRef,
    call: &ResolvedCall<T>,
    target: &CallTarget,
    arguments: &ast::ArgList,
    offset: TextSize,
    source_start: u32,
) -> anyhow::Result<Option<SignatureHelp>> {
    let db = analysis.view_db();
    let renderer = SignatureRenderer::new(db.crate_edition(crate_ref)?);
    let (signature, is_variadic) = match target {
        CallTarget::Function(function) => {
            let members = MemberView::new(db);
            let Some(function) = members
                .function(*function)
                .context("read signature help function")?
            else {
                return Ok(None);
            };
            (
                renderer.call_signature(function, call.param_offset()),
                function.is_variadic(),
            )
        }
        CallTarget::Value(ty) => {
            let Some(signature) = renderer.callable_signature(db, ty)? else {
                return Ok(None);
            };
            (signature, false)
        }
    };
    let CallSignature { label, parameters } = signature;

    // Count the commas in this list before the cursor. The comma in `(1, 2)` belongs to the
    // tuple. The parser wraps the comma in `send(, $0)` in an error node. That comma still
    // ends the first slot.
    //
    // Body IR skips cfg-disabled arguments. In `send(#[cfg(any())] (), $0value)`, the next
    // lowered span belongs to `value`. Leave it in the iterator and skip the comma after
    // `()` so `value` still gets the first parameter.
    let mut lowered = call.argument_spans().iter().copied().peekable();
    let mut disabled_argument = false;
    let mut argument_index = 0;
    for child in arguments
        .syntax()
        .children_with_tokens()
        .take_while(|child| child.text_range().end() <= offset)
    {
        let separator = child.kind() == SyntaxKind::COMMA
            || (child.kind() == SyntaxKind::ERROR
                && child
                    .as_node()
                    .and_then(|error| error.first_child_or_token())
                    .is_some_and(|token| token.kind() == SyntaxKind::COMMA));
        if separator {
            argument_index += usize::from(!disabled_argument);
            disabled_argument = false;
        } else if let Some(argument) = child.into_node().and_then(ast::Expr::cast) {
            let range = argument.syntax().text_range() + TextSize::from(source_start);
            let span = Span::from_text_range(range);
            // A macro-expanded argument can point to a token within its written expression.
            disabled_argument = lowered.next_if(|arg| span.contains_span(*arg)).is_none();
        }
    }
    // All extra arguments to a C variadic function share its trailing `...` slot.
    let active_parameter = parameters.len().checked_sub(1).and_then(|last| {
        (argument_index <= last || is_variadic).then_some(argument_index.min(last))
    });

    Ok(Some(SignatureHelp {
        label,
        parameters,
        active_parameter,
    }))
}
