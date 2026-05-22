use rg_item_tree::ItemTreeRef;

/// Stable identifier of one retained item produced by macro expansion.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct GeneratedItemId(pub usize);

/// Target-local reference to one generated item payload.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct GeneratedItemRef {
    pub item: GeneratedItemId,
}

/// Durable source identity for definitions collected into DefMap.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub enum ItemSource {
    ItemTree(ItemTreeRef),
    Generated(GeneratedItemRef),
}

impl ItemSource {
    /// Temporary migration boundary until Semantic IR lowers through an item-source reader.
    pub fn as_item_tree(self) -> Option<ItemTreeRef> {
        match self {
            Self::ItemTree(source) => Some(source),
            Self::Generated(_) => None,
        }
    }
}

impl From<ItemTreeRef> for ItemSource {
    fn from(source: ItemTreeRef) -> Self {
        Self::ItemTree(source)
    }
}
