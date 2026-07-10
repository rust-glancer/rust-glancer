//! Body IR snapshot storage and lazy package access.

mod db;
mod package;
mod txn;

pub use self::{
    db::{BodyIrDb, BodyIrStats},
    package::{
        BodyFileEntry, BodyFileShard, PackageBodies, PackageBodiesManifest, TargetBodies,
        TargetBodiesCoverage, TargetBodiesManifest, TargetBodiesStatus,
    },
    txn::{BodyIrLoader, BodyIrReadTxn, LoadBodyIr},
};
