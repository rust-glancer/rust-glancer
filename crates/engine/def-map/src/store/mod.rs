//! Def-map snapshot storage, lazy package access, and memory accounting.

mod cache;
mod db;
mod memsize;
mod txn;

pub use self::{
    cache::DefMapPackageBundle,
    db::{DefMapDb, DefMapStats},
    txn::DefMapReadTxn,
};
