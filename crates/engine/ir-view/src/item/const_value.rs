//! Const initializer details projected from semantic item storage.

use rg_ir_model::ConstRef;
use rg_semantic_ir::ItemStoreQuery;

use crate::IndexedViewDb;

/// Show short initializer source, adding its evaluated value when the rendered forms differ.
/// Crate-owned consts use Semantic IR; body-local and current consts use their local ItemStore.
/// Reading either kind uses stored data without running the evaluator again.
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
            .and_then(|data| {
                let value = data.value.as_deref().map(ToString::to_string);
                match (data.initializer_preview.as_deref(), value) {
                    // Keep meaningful source notation such as `7 * 60 + 17` or `0xff` alongside
                    // its decimal result. A literal already displayed as `2` needs no `// 2`.
                    (Some(preview), Some(value)) if preview != value => {
                        Some(format!("{preview}; // {value}"))
                    }
                    (Some(preview), _) => Some(preview.to_owned()),
                    (None, value) => value,
                }
            }))
    }
}
