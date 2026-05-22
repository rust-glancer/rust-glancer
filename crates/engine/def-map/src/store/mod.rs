//! Def-map snapshot storage and lazy package access.

mod db;
mod txn;

pub use self::{
    db::{DefMapDb, DefMapStats},
    txn::DefMapReadTxn,
};
