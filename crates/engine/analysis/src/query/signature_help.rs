//! Signature help for calls in the editor's source.

use anyhow::Context as _;
use rg_ir_model::{CrateRef, FileId};
use rg_ir_view::{
    display::signature::{CallSignature, SignatureRenderer},
    member::MemberView,
    ty::locals::{BodyView, CallTarget},
};
use rg_parse::syntax_edition;
use rg_std::ExpectedUnique;
use rg_syntax::{AstNode as _, TextRange, TextSize, ast};

use super::{active_call::ActiveCall, copied_call};
use crate::{Analysis, SignatureHelp};

pub(crate) fn signature_help(
    analysis: &Analysis<'_>,
    crate_ref: CrateRef,
    file_id: FileId,
    offset: u32,
) -> anyhow::Result<Option<SignatureHelp>> {
    let source = analysis.source_syntax(crate_ref, file_id)?;
    let offset = TextSize::from(offset);
    let Some(token) = ActiveCall::token_at(source.syntax(), offset) else {
        return Ok(None);
    };

    // In `send(forward!(parse($0)))`, a copied call can own the cursor inside a written call.
    if token
        .parent_ancestors()
        .any(|node| ast::TokenTree::can_cast(node.kind()))
    {
        let db = analysis.view_db();
        let mut calls = BodyView::new(db).copied_calls_at(crate_ref, file_id, offset.into())?;
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
                .filter(|list| ActiveCall::contains_offset(list, local_offset))
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
                let (argument_index, _) = ActiveCall::argument_at(
                    &arguments,
                    call.argument_spans(),
                    local_offset,
                    span.start,
                );
                help.push(render(
                    analysis,
                    crate_ref,
                    target,
                    call.param_offset(),
                    argument_index,
                )?);
            }
        }
        if selected_len.is_some() {
            return Ok(help.into_option().flatten());
        }
    }

    let Some(active) = ActiveCall::at(analysis, crate_ref, file_id, &token, offset)? else {
        return Ok(None);
    };
    let call = active.call();
    render(
        analysis,
        crate_ref,
        call.target(),
        call.param_offset(),
        active.argument_index(),
    )
}

fn render(
    analysis: &Analysis<'_>,
    crate_ref: CrateRef,
    target: &CallTarget,
    param_offset: usize,
    argument_index: usize,
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
                renderer.call_signature(function, param_offset),
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
