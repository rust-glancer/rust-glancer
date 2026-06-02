mod build;
mod cursor;
mod ir;
mod item_store_lowering;
mod store;

#[cfg(test)]
mod tests;

pub use self::{
    cursor::SemanticCursorCandidate,
    ir::{PackageIr, SemanticIrStats},
    item_store_lowering::{ItemStoreLowerer, ItemStoreSourceReader},
    store::{SemanticIrDb, SemanticIrReadTxn},
};
