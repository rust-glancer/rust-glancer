//! Mechanical lowering from AST expression bodies into Body IR.
//!
//! This pass does not resolve names. It records the source shape, lexical scopes,
//! and visibility-order binding boundaries so the later resolution pass can stay focused.

mod body;
mod builder;
mod builtin_macro;
mod crate_lowering;
mod declaration;
mod expr;
mod macro_expansion;
mod pat;
mod stmt;
mod syntax;
mod syntax_kind;
mod task;

use rg_arena::Arena;
use rg_ir_model::BodyId;

pub(super) use self::{
    builder::{LoweredBodyData, PendingBindingResolution},
    crate_lowering::CrateLowering,
    declaration::{CurrentDeclarationBuilder, CurrentRootItems},
    macro_expansion::BodyMacroExpansion,
    task::{BodyLoweringTask, BodyTaskLowering, BodyTaskSource, LoweredBodyTask},
};
/// Crate-local bodies that still retain source-ambiguous pattern binding candidates.
#[derive(Default)]
pub(super) struct LoweredCrateBodies {
    bodies: Arena<BodyId, LoweredBodyData>,
}

impl LoweredCrateBodies {
    pub(super) fn bodies(&self) -> &Arena<BodyId, LoweredBodyData> {
        &self.bodies
    }

    pub(super) fn bodies_mut(&mut self) -> &mut Arena<BodyId, LoweredBodyData> {
        &mut self.bodies
    }

    pub(super) fn alloc_body(&mut self, body: LoweredBodyData) -> BodyId {
        self.bodies.alloc(body)
    }

    pub(super) fn into_bodies(self) -> Arena<BodyId, LoweredBodyData> {
        self.bodies
    }
}
