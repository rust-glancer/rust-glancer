//! Chooses the most specific analysis symbol at one source offset.
//!
//! Body IR owns expression and binding spans, while def-map and semantic IR own item signatures.
//! This file merges those candidate streams before higher-level queries decide what to do with the
//! selected symbol.

use rg_body_ir::BodyCursorCandidate;
use rg_def_map::TargetRef;
use rg_parse::FileId;

use super::{
    Analysis, cursor,
    data::{SymbolAt, SymbolCandidate},
};

pub(super) struct SymbolFinder<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> SymbolFinder<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(super) fn symbol_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<SymbolAt>> {
        let mut candidates = Vec::new();
        candidates.extend(self.body_symbol_candidates(target, file_id, offset)?);
        candidates.extend(cursor::item_signature_candidates(
            self.0, target, file_id, offset,
        )?);

        // Overlapping syntax is common around type paths and expressions. The narrowest span is
        // the best proxy for the thing the user actually placed the cursor on.
        let symbol = candidates
            .into_iter()
            .min_by_key(|candidate| candidate.span.len())
            .map(|candidate| candidate.symbol);
        Ok(symbol)
    }

    fn body_symbol_candidates(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<SymbolCandidate>> {
        let candidates = self
            .0
            .body_ir
            .cursor_candidates(target, file_id, offset)?
            .into_iter()
            .map(body_cursor_candidate_to_symbol_candidate)
            .collect();
        Ok(candidates)
    }
}

fn body_cursor_candidate_to_symbol_candidate(candidate: BodyCursorCandidate) -> SymbolCandidate {
    let span = candidate.span();
    let symbol = match candidate {
        BodyCursorCandidate::Body { body, .. } => SymbolAt::Body { body },
        BodyCursorCandidate::Binding { body, binding, .. } => SymbolAt::Binding { body, binding },
        BodyCursorCandidate::Expr { body, expr, .. } => SymbolAt::Expr { body, expr },
        BodyCursorCandidate::LocalItem { item, span } => SymbolAt::LocalItem { item, span },
        BodyCursorCandidate::TypePath {
            body,
            scope,
            path,
            span,
        } => SymbolAt::BodyPath {
            body,
            scope,
            path,
            span,
        },
        BodyCursorCandidate::ValuePath {
            body,
            scope,
            path,
            span,
        } => SymbolAt::BodyValuePath {
            body,
            scope,
            path,
            span,
        },
    };

    SymbolCandidate { symbol, span }
}
