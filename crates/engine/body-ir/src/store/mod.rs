//! Body IR snapshot storage, lazy package access, and memory accounting.

mod db;
mod memsize;
mod txn;

pub use self::{db::BodyIrDb, txn::BodyIrReadTxn};
