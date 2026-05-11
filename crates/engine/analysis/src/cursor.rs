//! Collects item-signature cursor candidates for analysis queries.
//!
//! Def-map can see module definitions and use paths; semantic IR can see fields, functions, and
//! type paths inside signatures. Analysis keeps the final `SymbolAt` vocabulary here.

use rg_def_map::{DefMapCursorCandidate, TargetRef};
use rg_parse::FileId;
use rg_semantic_ir::SemanticCursorCandidate;

use super::{
    Analysis,
    data::{SymbolAt, SymbolCandidate},
};

pub(super) fn item_signature_candidates(
    analysis: &Analysis<'_>,
    target: TargetRef,
    file_id: FileId,
    offset: u32,
) -> anyhow::Result<Vec<SymbolCandidate>> {
    let mut candidates = Vec::new();

    CursorScanner {
        analysis,
        target,
        file_id,
        offset,
        candidates: &mut candidates,
    }
    .scan()?;

    Ok(candidates)
}

struct CursorScanner<'a, 'db> {
    analysis: &'a Analysis<'db>,
    target: TargetRef,
    file_id: FileId,
    offset: u32,
    candidates: &'a mut Vec<SymbolCandidate>,
}

impl CursorScanner<'_, '_> {
    fn scan(&mut self) -> anyhow::Result<()> {
        // Query both signature-level sources before `symbol_at` applies its smallest-span policy.
        self.scan_def_map_items()?;
        self.scan_semantic_items()?;
        Ok(())
    }

    fn scan_def_map_items(&mut self) -> anyhow::Result<()> {
        let candidates =
            self.analysis
                .def_map
                .cursor_candidates(self.target, self.file_id, self.offset)?;
        for candidate in candidates {
            match candidate {
                DefMapCursorCandidate::Def { def, span } => {
                    self.candidates.push(SymbolCandidate {
                        symbol: SymbolAt::Def { def, span },
                        span,
                    });
                }
                DefMapCursorCandidate::UsePath { module, path, span } => {
                    self.candidates.push(SymbolCandidate {
                        symbol: SymbolAt::UsePath { module, path, span },
                        span,
                    });
                }
            }
        }

        Ok(())
    }

    fn scan_semantic_items(&mut self) -> anyhow::Result<()> {
        let candidates = self.analysis.semantic_ir.signature_cursor_candidates(
            self.target,
            self.file_id,
            self.offset,
        )?;
        for candidate in candidates {
            match candidate {
                SemanticCursorCandidate::Field { field, span } => {
                    self.candidates.push(SymbolCandidate {
                        symbol: SymbolAt::Field { field, span },
                        span,
                    });
                }
                SemanticCursorCandidate::Function { function, span } => {
                    self.candidates.push(SymbolCandidate {
                        symbol: SymbolAt::Function { function, span },
                        span,
                    });
                }
                SemanticCursorCandidate::EnumVariant { variant, span } => {
                    self.candidates.push(SymbolCandidate {
                        symbol: SymbolAt::EnumVariant { variant, span },
                        span,
                    });
                }
                SemanticCursorCandidate::TypePath {
                    context,
                    path,
                    span,
                } => {
                    self.candidates.push(SymbolCandidate {
                        symbol: SymbolAt::TypePath {
                            context,
                            path,
                            span,
                        },
                        span,
                    });
                }
            }
        }

        Ok(())
    }
}
