//! Mechanical lowering from AST expression bodies into Body IR.
//!
//! This pass does not resolve names. It records the source shape, lexical scopes,
//! and visibility-order binding boundaries so the later resolution pass can stay focused.

mod arguments;
mod body;
mod builder;
mod builtin_macro;
mod expr;
mod macro_expansion;
mod pat;
mod stmt;
mod syntax;
mod syntax_kind;
mod task;

use rg_arena::Arena;
use rg_def_map::MacroCopyCoverage;
use rg_ir_model::{BodyId, BodyKey};

use self::arguments::ArgumentCopies;
pub(super) use self::{
    builder::{LoweredBodyData, PendingBindingResolution},
    macro_expansion::BodyMacroExpansion,
    task::{BodyLoweringTask, BodyTaskLowering, BodyTaskSource, GeneratedBodyTask},
};
/// The growing owner worklist, retaining source-ambiguous bindings until preparation settles them.
pub(super) struct LoweredBodies {
    bodies: Arena<BodyId, LoweredBodyData>,
    argument_copies: ArgumentCopies,
}

impl LoweredBodies {
    pub(super) fn new(key: BodyKey) -> Self {
        Self {
            bodies: Arena::new(),
            argument_copies: ArgumentCopies::new(key),
        }
    }

    pub(super) fn bodies(&self) -> &Arena<BodyId, LoweredBodyData> {
        &self.bodies
    }

    pub(super) fn bodies_mut(&mut self) -> &mut Arena<BodyId, LoweredBodyData> {
        &mut self.bodies
    }

    pub(super) fn alloc_body(&mut self, body: LoweredBodyData) -> BodyId {
        self.bodies.alloc(body)
    }

    pub(super) fn into_parts(self) -> (Arena<BodyId, LoweredBodyData>, Vec<MacroCopyCoverage>) {
        (self.bodies, self.argument_copies.into_expansions())
    }
}
