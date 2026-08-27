//! Semantic IR snapshot storage, lazy package access, and memory accounting.

mod db;
mod lazy;
mod loader;
mod txn;

pub use self::{
    db::SemanticIrDb,
    loader::{LoadSemanticIr, SemanticIrLoader},
    txn::SemanticIrReadTxn,
};

pub(crate) use self::db::SemanticIrDbMutator;
