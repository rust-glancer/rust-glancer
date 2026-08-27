//! Def-map snapshot storage and lazy package access.

mod db;
mod lazy;
mod loader;
mod txn;

pub use self::{
    db::{DefMapDb, DefMapStats, UnresolvedImportStats},
    loader::{DefMapLoader, LoadDefMap},
    txn::DefMapReadTxn,
};
