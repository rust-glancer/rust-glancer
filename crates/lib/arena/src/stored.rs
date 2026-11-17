//! Publish an arena as independently readable groups of records.

use std::marker::PhantomData;

use ajika::{Db, Services};
use wincode::{SchemaRead, SchemaWrite};

use crate::{Arena, ArenaId};

/// An arena whose records are acquired in groups, without changing their allocated IDs.
///
/// A point read loads the group containing the requested record. For example, `Id(130)` can
/// live in a group starting at index 128; it keeps its original ID rather than becoming `Id(2)`.
/// The publisher records each group's first index, preserving the IDs allocated by `Arena`.
/// Groups are only storage boundaries; readers do not need to know their size. Handles retain
/// no decoded records between operations.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct StoredArena<Id, T> {
    len: usize,
    groups: Vec<(usize, ajika::Output<Vec<T>>)>,
    marker: PhantomData<fn(Id) -> Id>,
}

impl<Id: ArenaId, T: ajika::Persist + PartialEq + Send + Sync + 'static> StoredArena<Id, T> {
    /// Publish a completed arena from inside an Ajika query.
    ///
    /// Use a different `name` for each arena published by the query. The returned handles become
    /// readable once that query finishes, so the producer must finish its own record reads first.
    pub fn publish(
        db: &Db<'_, impl Services>,
        name: &str,
        arena: Arena<Id, T>,
    ) -> ajika::Result<Self> {
        // Grouping amortizes output metadata without making a point read acquire the entire
        // arena. Boundaries are persisted, so readers do not depend on this size.
        const GROUP_LEN: usize = 128;
        let len = arena.len();
        let mut records = arena.into_vec().into_iter();
        let mut groups = Vec::new();
        for start in (0..len).step_by(GROUP_LEN) {
            let values: Vec<_> = records.by_ref().take(GROUP_LEN).collect();
            groups.push((start, db.output((name, start), values.into())?));
        }
        Ok(Self {
            len,
            groups,
            marker: PhantomData,
        })
    }

    /// Enumerate allocated identities without acquiring their records.
    pub fn ids(&self) -> impl Iterator<Item = Id> {
        (0..self.len).map(Id::from_index)
    }

    /// Acquire the containing group, including its neighboring records, and select this ID.
    pub fn get<'db>(&self, db: &Db<'db, impl Services>, id: Id) -> ajika::Result<Option<&'db T>> {
        let index = id.index();
        if index >= self.len {
            return Ok(None);
        }
        let group = self.groups.partition_point(|(start, _)| *start <= index) - 1;
        let (start, output) = &self.groups[group];
        Ok(output.value_ref(db)?.get(index - start))
    }

    /// Whole-arena consumers explicitly acquire every group; point reads use `get`.
    pub fn entries<'db>(&self, db: &Db<'db, impl Services>) -> ajika::Result<Vec<(Id, &'db T)>> {
        let mut entries = Vec::with_capacity(self.len);
        for (start, output) in &self.groups {
            entries.extend(
                output
                    .value_ref(db)?
                    .iter()
                    .enumerate()
                    .map(|(offset, value)| (Id::from_index(start + offset), value)),
            );
        }
        Ok(entries)
    }
}
