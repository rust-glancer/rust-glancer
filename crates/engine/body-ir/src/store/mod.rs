//! Body IR snapshot storage and lazy package access.

mod current_body;
mod db;
mod package;
mod txn;

pub use self::{
    current_body::{CurrentBody, CurrentBodySet},
    db::{BodyIrDb, BodyIrStats},
    package::{
        BodyFileEntry, BodyFileShard, BodyLocalItems, CrateBodies, CrateBodiesCoverage,
        CrateBodiesManifest, CrateBodiesStatus, PackageBodies, PackageBodiesManifest,
    },
    txn::{BodyIrLoader, BodyIrReadTxn, LoadBodyIr},
};
