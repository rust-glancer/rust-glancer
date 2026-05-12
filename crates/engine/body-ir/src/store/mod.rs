//! Body IR snapshot storage, lazy package access, and memory accounting.

mod cache;
mod db;
mod memsize;
mod txn;

pub use self::{cache::BodyIrPackageBundle, db::BodyIrDb, txn::BodyIrReadTxn};
