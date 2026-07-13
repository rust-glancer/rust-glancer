//! Body IR snapshot storage and lazy package access.

mod db;
mod package;
mod txn;

pub use self::{
    db::{BodyIrDb, BodyIrStats},
    package::{
        BodyFileEntry, BodyFileShard, BodyLocalItems, CrateBodies, CrateBodiesCoverage,
        CrateBodiesManifest, CrateBodiesStatus, PackageBodies, PackageBodiesManifest,
    },
    txn::{BodyIrLoader, BodyIrReadTxn, LoadBodyIr},
};
