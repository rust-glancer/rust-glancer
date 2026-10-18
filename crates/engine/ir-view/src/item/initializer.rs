//! Const and static initializer details projected from semantic item storage.

use rg_ir_model::ConstOrStaticRef;
use rg_semantic_ir::ItemStoreQuery;

use crate::IndexedViewDb;

/// Show short initializer source, adding its evaluated value when the rendered forms differ.
/// Crate-owned declarations use Semantic IR; body-local and current ones use their local ItemStore.
/// Reading either kind uses stored data without running the evaluator again.
pub struct InitializerView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> InitializerView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub fn render(&self, item: ConstOrStaticRef) -> anyhow::Result<Option<String>> {
        let items = ItemStoreQuery::new(self.db);
        let (preview, value) = match item {
            ConstOrStaticRef::Const(item) => {
                let Some(data) = items.const_data(item)? else {
                    return Ok(None);
                };
                (data.initializer_preview.as_deref(), data.value.as_deref())
            }
            ConstOrStaticRef::Static(item) => {
                let Some(data) = items.static_data(item)? else {
                    return Ok(None);
                };
                (data.initializer_preview.as_deref(), data.value.as_deref())
            }
        };
        let value = value.map(ToString::to_string);
        Ok(match (preview, value) {
            // Keep meaningful source notation such as `7 * 60 + 17` or `0xff` alongside
            // its decimal result. A literal already displayed as `2` needs no `// 2`.
            (Some(preview), Some(value)) if preview != value => {
                Some(format!("{preview}; // {value}"))
            }
            (Some(preview), _) => Some(preview.to_owned()),
            (None, value) => value,
        })
    }
}
