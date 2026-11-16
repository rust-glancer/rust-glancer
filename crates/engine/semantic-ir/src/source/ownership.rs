//! Source membership is a declaration result, not a side effect of opening a file.

use std::collections::BTreeMap;

use rg_ir_model::{CrateRef, FileId, ItemStoreRef};
use rg_source::QueryContext;
use wincode::{SchemaRead, SchemaWrite};

use crate::{associated_declarations, target_declarations};

/// Ownership known after module collection, plus owners whose macros may add files.
///
/// Absence from `known_owners` does not prove that a file is detached. This partial index is what
/// startup publishes; only `complete_source_ownership` can produce exhaustive membership.
#[derive(Debug, Default, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct SourceOwnershipIndex {
    known: BTreeMap<FileId, Vec<ItemStoreRef>>,
    pending: Vec<ItemStoreRef>,
}

impl SourceOwnershipIndex {
    pub fn known_owners(&self, file: FileId) -> &[ItemStoreRef] {
        self.known.get(&file).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn record(&mut self, file: FileId, owner: ItemStoreRef) {
        let owners = self.known.entry(file).or_default();
        if !owners.contains(&owner) {
            owners.push(owner);
        }
    }

    pub(crate) fn defer(&mut self, owner: ItemStoreRef) {
        self.pending.push(owner);
    }
}

/// Exhaustive declaration ownership for one target. This can prove that a file has no owner.
///
/// The scope store covers module declarations throughout the target. Each trait or impl has
/// its own member store, so repeated includes keep all of those interpretations. Capturing a
/// source input alone establishes neither relationship. This does not describe non-declaration
/// dependencies such as files read by a macro without emitting any items from them.
#[derive(Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct SourceOwnership {
    owners: BTreeMap<FileId, Vec<ItemStoreRef>>,
}

impl SourceOwnership {
    pub fn owners(&self, file: FileId) -> &[ItemStoreRef] {
        self.owners.get(&file).map_or(&[], Vec::as_slice)
    }

    pub fn files(&self) -> impl Iterator<Item = FileId> + '_ {
        self.owners.keys().copied()
    }
}

/// Discover all files introduced by member expansion. Cached outputs retain these relationships
/// across operations and restarts; replay never needs a registration callback to restore them.
/// This is demanded by exhaustive scans or routing an unknown file, never by startup.
#[ajika::query(returns(ref))]
pub fn complete_source_ownership(
    db: &QueryContext<'_>,
    krate: CrateRef,
) -> ajika::Result<SourceOwnership> {
    let target = target_declarations(db, krate)?;
    let known = target.source_ownership.value_ref(db)?;
    let discovered = db.parallel_map(&known.pending, |db, &owner| {
        let declarations = associated_declarations(db, krate, owner.id)?
            .as_ref()
            .ok_or_else(|| ajika::Error::contract("source owner has no declarations"))?;
        Ok((owner, declarations.files.value(db)?))
    })?;
    let mut result = SourceOwnership {
        owners: known.known.clone(),
    };
    for (owner, files) in discovered {
        for &file in files.iter() {
            let owners = result.owners.entry(file).or_default();
            if !owners.contains(&owner) {
                owners.push(owner);
            }
        }
    }
    Ok(result)
}
