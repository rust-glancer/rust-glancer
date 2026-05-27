//! Shared read handle for indexed-data views.

use rg_body_ir::BodyIrReadTxn;
use rg_def_map::DefMapReadTxn;
use rg_semantic_ir::SemanticIrReadTxn;

/// Read-only database handle used by all indexed-data views.
///
/// The handle deliberately contains the concrete frozen storage transactions. That keeps views
/// easy to extract as one crate first; a trait facade can replace these fields later once the
/// method surface settles.
#[derive(Debug, Clone)]
pub(crate) struct IndexedViewDb<'db> {
    pub(crate) def_map: DefMapReadTxn<'db>,
    pub(crate) semantic_ir: SemanticIrReadTxn<'db>,
    pub(crate) body_ir: BodyIrReadTxn<'db>,
}

impl<'db> IndexedViewDb<'db> {
    pub(crate) fn new(
        def_map: DefMapReadTxn<'db>,
        semantic_ir: SemanticIrReadTxn<'db>,
        body_ir: BodyIrReadTxn<'db>,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            body_ir,
        }
    }
}
