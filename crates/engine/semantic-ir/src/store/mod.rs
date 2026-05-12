//! Semantic IR snapshot storage, lazy package access, and memory accounting.

mod cache;
mod db;
mod memsize;
mod txn;

pub use self::{cache::SemanticIrPackageBundle, db::SemanticIrDb, txn::SemanticIrReadTxn};

pub(crate) use self::db::SemanticIrDbMutator;
