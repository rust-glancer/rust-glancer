//! Request-local loop-label scope over indexed body expressions.
//!
//! Body IR stores a label on the expression that declares it, but it does not retain a separate
//! "jump targets visible at this offset" index. This scanner rebuilds that small view only for the
//! active request by looking at the written expressions that enclose the cursor.

use rg_body_ir::{BodyIrReadTxn, ExprKind};
use rg_ir_model::CrateRef;
use rg_package_store::PackageStoreError;
use rg_parse::FileId;
use rg_std::UniqueVec;

/// Finds the jump targets visible from one `break` or `continue` position.
///
/// ```text
/// 'outer: loop {
///     'inner: while ready() {
///         break '$0
///     }
/// }
/// ```
///
/// The result is `["'inner", "'outer"]`: narrower source spans come first, and a shadowed label
/// spelling appears only once.
pub(crate) struct LabelScopeScanner<'txn, 'db> {
    body_ir: &'txn BodyIrReadTxn<'db>,
    crate_ref: CrateRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> LabelScopeScanner<'txn, 'db> {
    pub(crate) fn new(
        body_ir: &'txn BodyIrReadTxn<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> Self {
        Self {
            body_ir,
            crate_ref,
            file_id,
            offset,
        }
    }

    /// Return enclosing labels from the nearest target outward.
    pub(crate) fn labels(&self) -> Result<Vec<String>, PackageStoreError> {
        let mut labels = Vec::new();
        for (_, body) in self.body_ir.bodies(self.crate_ref, Some(self.file_id))? {
            if !body.source().span.touches(self.offset) {
                continue;
            }
            for expr in body.exprs() {
                if !expr.source.is_written_in_file(self.file_id)
                    || !expr.source.span.touches(self.offset)
                {
                    continue;
                }
                let label = match &expr.kind {
                    ExprKind::Block { label, .. }
                    | ExprKind::Loop { label, .. }
                    | ExprKind::While { label, .. }
                    | ExprKind::For { label, .. } => label.as_ref(),
                    _ => None,
                };
                let Some(label) = label else {
                    continue;
                };
                labels.push((expr.source.span.len(), label.name.to_string()));
            }
        }

        // The narrowest enclosing expression is the nearest jump target. Deduplicate names after
        // sorting so a shadowing inner label wins deterministically.
        labels.sort_by_key(|(source_len, _)| *source_len);
        let mut result = UniqueVec::new();
        for (_, label) in labels {
            result.push(label);
        }
        Ok(result.into_vec())
    }
}
