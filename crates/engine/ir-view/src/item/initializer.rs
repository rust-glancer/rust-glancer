//! Join initializer source previews and explicitly requested body values for display.

use rg_ir_model::ConstOrStaticRef;
use rg_semantic_ir::ItemStoreQuery;

use crate::IndexedViewDb;

/// Show short initializer source, adding its evaluated value when the rendered forms differ.
/// Declaration reads supply only identity and a source occurrence. Values are demanded separately
/// from body facts, so ordinary indexing and declared-type queries never evaluate initializers.
pub struct InitializerView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> InitializerView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub fn render(&self, item: ConstOrStaticRef) -> anyhow::Result<Option<String>> {
        let items = ItemStoreQuery::new(self.db);
        let (source, location) = match item {
            ConstOrStaticRef::Const(item) => {
                let Some(data) = items.const_data(item)? else {
                    return Ok(None);
                };
                (data.source, data.initializer_location)
            }
            ConstOrStaticRef::Static(item) => {
                let Some(data) = items.static_data(item)? else {
                    return Ok(None);
                };
                (data.source, data.initializer_location)
            }
        };
        let preview = if let Some(current) = self.db.body_ir.current_initializer(item) {
            current.initializer_preview()
        } else if let Some(location) = location {
            self.db
                .body_ir
                .declaration_source(item.origin().origin_crate(), source)?
                .initializer_preview(location)
        } else {
            None
        };
        let value = self
            .db
            .body_ir
            .initializer_value(item)?
            .map(|value| value.to_string());
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
