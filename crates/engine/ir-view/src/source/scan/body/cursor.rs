//! Point-query scanning for source requests at one offset.
//!
//! Point queries pick the most specific body-local node under the cursor, then
//! add any extra path-segment candidates visible at the same offset.
//!
//! ```text
//! items.push$0(value)
//!       ^^^^ select the method name, not the enclosing call or block
//!
//! Action::Sta$0rt
//!         ^^^^^ keep a candidate for this path segment as well
//! ```
//!
//! The first candidate gives editor queries the smallest AST-like body node. Path scanners add the
//! second lane because one lowered expression can contain several independently navigable path
//! segments.

use rg_body_ir::{BodyOwner, BodyView};
use rg_ir_model::{
    BodyRef, CrateRef, DefMapRef, ExprId, FileId, SemanticItemRef, SourceLocation,
    SourceLocationKind, identity::DeclarationRef,
};
use rg_item_tree::SourceLocations;
use rg_semantic_ir::SemanticItemSource as _;
use rg_source::ReadError;

use super::{
    BodySourceCandidate, RecordFieldKeySurface,
    declarations::BodyDeclarationScanner,
    paths::{BodyPathSourceScanner, TypePathSourceScanner},
    sites::BodyScanSites,
};
use crate::{IndexedViewDb, source::scan::NarrowestSourceSite};

/// Scans one Body IR transaction for all source interpretations at a cursor offset.
///
/// Ordinary source selects one narrowest body-local candidate plus any type/value path segments
/// that touch the same offset. If a macro copied the selected token into several nodes, keep each
/// expanded use. Analysis can then interpret all uses without Body IR choosing a navigation target.
pub(crate) struct BodyCursorScanner<'txn, 'db> {
    db: &'txn IndexedViewDb<'db>,
    crate_ref: CrateRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> BodyCursorScanner<'txn, 'db> {
    pub(crate) fn new(
        db: &'txn IndexedViewDb<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> Self {
        Self {
            db,
            crate_ref,
            file_id,
            offset,
        }
    }

    /// Returns body-local candidates that can answer an editor query at this exact offset.
    pub(crate) fn scan(&self) -> Result<Vec<BodySourceCandidate>, ReadError> {
        let mut candidates = Vec::new();
        let mut written = NarrowestSourceSite::new();
        for (body_ref, body) in
            self.db
                .body_ir
                .bodies_at(self.crate_ref, self.file_id, self.offset, None)?
        {
            let source = body.source();
            if source.file_id != self.file_id || !source.span.contains(self.offset) {
                continue;
            }
            if source.is_written() {
                written.consider((body_ref, body), source.span.len());
            } else {
                // Two functions emitted by `make!(run)` can both use the invocation as
                // their source range. Scan both bodies because each can give the same
                // written `run` a different meaning.
                self.scan_body(body_ref, body, &mut candidates)?;
            }
        }
        if let Some((body_ref, body)) = written.finish() {
            self.scan_body(body_ref, body, &mut candidates)?;
        }
        Ok(candidates)
    }

    fn scan_body(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        candidates: &mut Vec<BodySourceCandidate>,
    ) -> Result<(), ReadError> {
        candidates.extend(self.candidate_at_body(body_ref, body)?);
        TypePathSourceScanner::at(body_ref, body, self.file_id, self.offset, candidates).scan();
        BodyPathSourceScanner::at(body_ref, body, self.file_id, self.offset, candidates).scan();
        Ok(())
    }

    /// Pick the narrowest source span, retaining expanded uses that share that span.
    /// Outside macro calls, the body itself is the fallback when no smaller candidate matches.
    fn candidate_at_body(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
    ) -> Result<Vec<BodySourceCandidate>, ReadError> {
        let mut best = BestCursorCandidate::new(BodySourceCandidate::Body {
            body: body_ref,
            location: body.source().location(),
        });
        let sites = BodyScanSites::new(body);
        let record_shorthand_values = sites.record_expr_shorthand_value_ids();

        // The cursor can be on a nested fn/const/static name. Its body owns the initializer
        // or block, while its declaration still belongs to the parent body-local item store.
        self.consider_body_owner_declaration(body, &mut best)?;

        // Macro expansion can map many generated nodes back to the invocation span. Give the
        // written macro path a precise candidate before scanning the arguments. Helper invocations
        // created by a template have no written name token and stay hidden.
        self.consider_macro_calls(body_ref, body, &mut best);

        // Inside an expansion, a broad handwritten parent (such as the function block) must
        // not mask a mapped argument or answer for a generated helper. Precise argument tokens
        // are considered below, while ordinary parent expressions stay outside this boundary.
        let inside_expansion = self.macro_source_touches_offset(body);

        // First look at expressions under the cursor: calls, paths, field accesses, literals, and
        // so on. Record shorthand values are skipped here because the key token has its own
        // source-level candidate.
        for (expr_idx, expr) in body.exprs().iter().enumerate() {
            let source = body.source_at(expr.source);
            if source.file_id != self.file_id {
                continue;
            }
            let name = sites.expression_name(expr);
            if !source.is_written() {
                sites.walk_record_expr_keys(body_ref, expr, Some(self.offset), |candidate| {
                    best.consider_record_field(candidate);
                });
                let location = name.unwrap_or(source).location();
                if location.kind == SourceLocationKind::MacroArgument
                    && location.span.touches(self.offset)
                {
                    best.consider(BodySourceCandidate::Expr {
                        body: body_ref,
                        expr: ExprId(expr_idx),
                        file_id: source.file_id,
                        location,
                    });
                }
                continue;
            }
            if inside_expansion
                || !source.span.touches(self.offset)
                || record_shorthand_values.contains(&ExprId(expr_idx))
            {
                continue;
            }
            // A direct expression also provides its type when the cursor is on punctuation.
            sites.walk_record_expr_keys(body_ref, expr, Some(self.offset), |candidate| {
                best.consider_record_field(candidate);
            });
            let location = name
                .filter(|name| name.span.touches(self.offset))
                .unwrap_or(source)
                .location();
            best.consider(BodySourceCandidate::Expr {
                body: body_ref,
                expr: ExprId(expr_idx),
                file_id: source.file_id,
                location,
            });
        }

        // Record pattern keys are not expressions, so check them separately before ordinary
        // binding declarations.
        sites.walk_record_pat_keys(
            body_ref,
            Some(self.file_id),
            Some(self.offset),
            |candidate| {
                best.consider_record_field(candidate);
            },
        );

        BodyDeclarationScanner::new(
            self.db,
            body_ref,
            body,
            Some(self.file_id),
            Some(self.offset),
        )
        .scan(|candidate| {
            if matches!(candidate, BodySourceCandidate::Binding { .. }) {
                best.consider_mapped(candidate);
            } else {
                best.consider(candidate);
            }
        })?;

        if inside_expansion && best.candidates.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(best.finish())
        }
    }

    /// Projects a nested body back to its body-local item declaration when the cursor is on the
    /// owner name, so editor queries target the function/const/static item instead of the body.
    fn consider_body_owner_declaration(
        &self,
        body: BodyView<'_>,
        best: &mut BestCursorCandidate,
    ) -> Result<(), ReadError> {
        let Some((item, location)) = self.body_owner_declaration(body)? else {
            return Ok(());
        };
        if !location.span.touches(self.offset) {
            return Ok(());
        }

        best.consider(BodySourceCandidate::LocalDeclaration {
            declaration: DeclarationRef::from(item),
            location,
        });

        Ok(())
    }

    fn consider_macro_calls(
        &self,
        body_ref: BodyRef,
        body: BodyView<'_>,
        best: &mut BestCursorCandidate,
    ) {
        for call in body.macro_calls() {
            if let Some(candidate) =
                BodySourceCandidate::macro_call(body_ref, call, body.sources(), Some(self.file_id))
                && candidate.span().touches(self.offset)
            {
                best.consider(candidate);
            }
        }
    }

    fn body_owner_declaration(
        &self,
        body: BodyView<'_>,
    ) -> Result<Option<(SemanticItemRef, SourceLocation)>, ReadError> {
        let item = match body.owner() {
            BodyOwner::Function(function) => SemanticItemRef::from(function),
            BodyOwner::Const(konst) => SemanticItemRef::from(konst),
            BodyOwner::Static(static_ref) => SemanticItemRef::from(static_ref),
        };
        if !matches!(item.origin(), DefMapRef::Body(_)) {
            return Ok(None);
        }
        let Some(view) = self.db.item(item)? else {
            return Ok(None);
        };
        let locations = self
            .db
            .body_ir
            .declaration_source(item.origin().origin_crate(), view.source())?;
        Ok(locations
            .exact_location(view.name_location())
            .map(|location| (item, location)))
    }

    /// Recognize macro ranges even when they produce no expression, such as type expansions.
    fn macro_source_touches_offset(&self, body: BodyView<'_>) -> bool {
        body.macro_calls().iter().any(|call| {
            let source = body.source_at(call.source);
            source.file_id == self.file_id && source.span.touches(self.offset)
        }) || body.exprs().iter().any(|expr| {
            let source = body.source_at(expr.source);
            !source.is_written()
                && source.file_id == self.file_id
                && source.span.touches(self.offset)
        }) || body.pats().iter().any(|pat| {
            let source = body.source_at(pat.source);
            !source.is_written()
                && source.file_id == self.file_id
                && source.span.touches(self.offset)
        }) || body.bindings().iter().any(|binding| {
            let source = body.source_at(binding.source);
            !source.is_written()
                && source.file_id == self.file_id
                && source.span.touches(self.offset)
        })
    }
}

/// Tracks the narrowest body-local candidate seen while scanning one body.
///
/// For `both!(run)` expanding to `(First.run(), Second.run())`, both method calls have the same
/// source span. Keep both candidates so a cursor query can report both meanings of `run`.
struct BestCursorCandidate {
    candidates: Vec<BodySourceCandidate>,
    fallback: BodySourceCandidate,
}

impl BestCursorCandidate {
    fn new(fallback: BodySourceCandidate) -> Self {
        Self {
            candidates: Vec::new(),
            fallback,
        }
    }

    fn consider(&mut self, candidate: BodySourceCandidate) {
        if candidate.location().kind == SourceLocationKind::MacroArgument {
            self.consider_mapped(candidate);
        } else {
            self.consider_narrower(candidate);
        }
    }

    /// Direct shorthand keeps its value or binding as the cursor target. Copied shorthand
    /// can expose both meanings of the same input.
    fn consider_record_field(&mut self, candidate: BodySourceCandidate) {
        if candidate.location().kind == SourceLocationKind::MacroArgument
            || matches!(
                candidate,
                BodySourceCandidate::RecordFieldKey {
                    surface: RecordFieldKeySurface::Explicit,
                    ..
                }
            )
        {
            self.consider_mapped(candidate);
        }
    }

    fn consider_narrower(&mut self, candidate: BodySourceCandidate) {
        if self
            .candidates
            .first()
            .is_none_or(|best| candidate.span().len() < best.span().len())
        {
            self.candidates.clear();
            self.candidates.push(candidate);
        }
    }

    /// Keep distinct uses of the same original span; a narrower span still replaces them all.
    fn consider_mapped(&mut self, candidate: BodySourceCandidate) {
        if self
            .candidates
            .first()
            .is_some_and(|best| best.span() == candidate.span())
        {
            if !self.candidates.contains(&candidate) {
                self.candidates.push(candidate);
            }
        } else {
            self.consider_narrower(candidate);
        }
    }

    fn finish(self) -> Vec<BodySourceCandidate> {
        if self.candidates.is_empty() {
            vec![self.fallback]
        } else {
            self.candidates
        }
    }
}
