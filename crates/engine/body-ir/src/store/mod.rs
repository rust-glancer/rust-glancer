//! Body IR snapshot storage and lazy package access.

mod db;
mod txn;

pub use self::{db::BodyIrDb, txn::BodyIrReadTxn};
