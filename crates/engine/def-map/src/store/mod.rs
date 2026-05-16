//! Def-map snapshot storage, lazy package access, and memory accounting.

mod db;
mod memsize;
mod txn;

pub use self::{
    db::{DefMapDb, DefMapStats},
    txn::DefMapReadTxn,
};
