//! Body IR snapshot storage and lazy package access.

mod db;
mod package;
mod txn;

pub use self::{
    db::{BodyIrDb, BodyIrStats},
    package::{PackageBodies, TargetBodies, TargetBodiesCoverage, TargetBodiesStatus},
    txn::BodyIrReadTxn,
};
