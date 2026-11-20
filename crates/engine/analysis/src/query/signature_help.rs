//! Signature help for calls in the editor's source.

use anyhow::Context as _;
use rg_ir_model::{CrateRef, FileId};
use rg_ir_view::{
    display::signature::{CallSignature, SignatureRenderer},
    member::MemberView,
    ty::locals::CallTarget,
};

use super::active_call::ActiveCall;
use crate::{Analysis, SignatureHelp};

pub(crate) fn signature_help(
    analysis: &Analysis<'_>,
    crate_ref: CrateRef,
    file_id: FileId,
    offset: u32,
) -> anyhow::Result<Option<SignatureHelp>> {
    let Some(active) = ActiveCall::at(analysis, crate_ref, file_id, offset)? else {
        return Ok(None);
    };
    let db = analysis.view_db();
    let call = active.call();
    let renderer = SignatureRenderer::new(db.crate_edition(crate_ref)?);
    let (signature, is_variadic) = match call.target() {
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

    let argument_index = active.argument_index();
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
