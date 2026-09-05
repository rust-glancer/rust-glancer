//! Body IR snapshot storage and lazy package access.

pub(crate) mod current;
mod db;
mod package;
mod txn;

pub(crate) use self::current::CurrentBody;

pub use self::{
    current::CurrentSourceStore,
    db::{BodyIrDb, BodyIrStats},
    package::{
        BodyFileEntry, BodyFileShard, BodyLocalItems, CrateBodies, CrateBodiesCoverage,
        CrateBodiesManifest, CrateBodiesStatus, PackageBodies, PackageBodiesCoverage,
        PackageBodiesManifest,
    },
    txn::{BodyIrLoader, BodyIrReadTxn, LoadBodyIr},
};
