use std::{collections::HashSet, fmt};

use rg_ir_model::{
    BodyKey, BodyRef, ConstOrStaticRef, CrateRef, EvaluatedConstValue, FileId, SavedBodyOwner,
};
use rg_semantic_ir::{BodyOwnerData, target_declarations};
use rg_source::ReadError;
use rg_std::UniqueVec;

use super::BodySource;
use crate::{BodyLocalItems, BodyView, SavedBody, saved_body};

/// Read saved bodies for the targets in this view. The owner catalog lets file enumeration
/// find bodies without computing them.
pub(super) struct SavedBodyReader<'db> {
    db: &'db rg_source::QueryContext<'db>,
    targets: HashSet<CrateRef>,
}

impl<'db> SavedBodyReader<'db> {
    pub(super) fn new(
        db: &'db rg_source::QueryContext<'db>,
        targets: impl Iterator<Item = CrateRef>,
    ) -> Self {
        Self {
            db,
            targets: targets.collect(),
        }
    }

    fn owners(&self, krate: CrateRef) -> Result<&'db [BodyOwnerData], ReadError> {
        if !self.targets.contains(&krate) {
            return Ok(&[]);
        }
        let declarations = target_declarations(self.db, krate).map_err(ReadError::Query)?;
        let owners = declarations
            .body_owners
            .value_ref(self.db)
            .map_err(ReadError::Query)?;
        Ok(owners.as_slice())
    }

    fn owner(&self, key: BodyKey) -> Result<Option<&'db SavedBody>, ReadError> {
        if !self.targets.contains(&key.krate) {
            return Ok(None);
        }
        Ok(saved_body(self.db, key).map_err(ReadError::Query)?.as_ref())
    }
}

impl BodySource for SavedBodyReader<'_> {
    fn files(&self, krate: CrateRef) -> Result<UniqueVec<FileId>, ReadError> {
        Ok(self.owners(krate)?.iter().map(|owner| owner.file).collect())
    }

    fn body(&self, body: BodyRef) -> Result<Option<(BodyView<'_>, &BodyLocalItems)>, ReadError> {
        let Some(owner) = body.root.owner() else {
            return Ok(None);
        };
        let key = BodyKey {
            krate: body.crate_ref,
            owner,
        };
        let Some(saved) = self.owner(key)? else {
            return Ok(None);
        };
        Ok(saved
            .bodies
            .body(body.body)
            .zip(saved.bodies.body_local_items(body.body)))
    }

    fn initializer(
        &self,
        item: ConstOrStaticRef,
    ) -> Result<Option<&EvaluatedConstValue>, ReadError> {
        let Some(krate) = item.origin().as_crate_ref() else {
            return Ok(None);
        };
        let owner = match item {
            ConstOrStaticRef::Const(item) => SavedBodyOwner::Const(item.id),
            ConstOrStaticRef::Static(item) => SavedBodyOwner::Static(item.id),
        };
        Ok(self
            .owner(BodyKey { krate, owner })?
            .and_then(|saved| saved.initializer.as_ref()))
    }

    fn bodies(
        &self,
        krate: CrateRef,
        file: Option<FileId>,
        masked: &dyn Fn(FileId) -> bool,
    ) -> Result<Vec<(BodyRef, BodyView<'_>)>, ReadError> {
        let mut bodies = Vec::new();
        for root in self
            .owners(krate)?
            .iter()
            .filter(|root| file.is_none_or(|file| root.file == file) && !masked(root.file))
        {
            let key = BodyKey {
                krate,
                owner: root.owner,
            };
            if let Some(owner) = self.owner(key)? {
                bodies.extend(
                    owner
                        .bodies
                        .body_views()
                        .map(|(id, view)| (key.body(id), view)),
                );
            }
        }
        Ok(bodies)
    }
}

impl fmt::Debug for SavedBodyReader<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SavedBodyReader")
            .field("targets", &self.targets.len())
            .finish_non_exhaustive()
    }
}
