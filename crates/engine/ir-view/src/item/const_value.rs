//! Evaluated initializer values projected from semantic item storage.

use rg_ir_model::ConstRef;
use rg_semantic_ir::ItemStoreQuery;

use crate::IndexedViewDb;

/// Format concrete results already recorded on const declarations.
/// Crate-owned consts use Semantic IR; body-local and current consts use their local ItemStore.
/// Reading either kind reuses the stored result without running the evaluator again.
pub struct ConstValueView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> ConstValueView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub fn render(&self, konst: ConstRef) -> anyhow::Result<Option<String>> {
        Ok(ItemStoreQuery::new(self.db)
            .const_data(konst)?
            .and_then(|data| data.value.as_deref())
            .map(ToString::to_string))
    }
}
