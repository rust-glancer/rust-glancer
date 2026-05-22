//! Semantic IR snapshot storage, lazy package access, and memory accounting.

mod db;
mod txn;

pub use self::{db::SemanticIrDb, txn::SemanticIrReadTxn};

pub(crate) use self::db::SemanticIrDbMutator;
