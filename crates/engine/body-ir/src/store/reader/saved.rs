use std::{collections::HashSet, fmt, sync::Arc};

use rg_def_map::{ItemSource, ItemSourceKind};
use rg_ir_model::{BodyKey, BodyRef, CrateRef, FileId};
use rg_profile::ProfileThreadContext;
use rg_semantic_ir::{BodyOwnerData, SavedDeclarationSourceView, target_declarations};
use rg_source::{QueryContext, ReadError};
use rg_std::UniqueVec;

use super::DeclarationSourceView;
use crate::{
    BodyData, BodyFactsStore, BodyLocalItems, BodyStore, BodyView, PreparedBodyStore,
    infer_saved_body, prepare_saved_body, saved_body,
};

/// Read saved bodies for the targets in this view. The owner catalog lets file enumeration
/// find bodies without computing them.
pub(super) struct SavedBodyReader<'db> {
    db: &'db QueryContext<'db>,
    targets: HashSet<CrateRef>,
}

impl<'db> SavedBodyReader<'db> {
    pub(super) fn new(db: &'db QueryContext<'db>, targets: impl Iterator<Item = CrateRef>) -> Self {
        Self {
            db,
            targets: targets.collect(),
        }
    }

    fn owner(&self, key: BodyKey) -> Result<Option<&'db BodyStore>, ReadError> {
        if !self.targets.contains(&key.krate) {
            return Ok(None);
        }
        Ok(saved_body(self.db, key).map_err(ReadError::Query)?.as_ref())
    }

    pub(crate) fn declaration_source(
        &self,
        krate: CrateRef,
        source: ItemSource,
    ) -> Result<DeclarationSourceView<'_>, ReadError> {
        if let ItemSourceKind::Body(source) = source.kind {
            // Resolving an occurrence only needs the location output. Keep this separate from
            // local declaration reads, which must observe prepared meaning alone.
            let sources = if let Some(key) = source.body.saved_key()
                && self.targets.contains(&source.body.crate_ref)
            {
                let prepared = prepare_saved_body(self.db, key).map_err(ReadError::Query)?;
                match prepared.as_ref() {
                    Some(prepared) => prepared
                        .sources
                        .value_ref(self.db)
                        .map_err(ReadError::Query)?
                        .get(source.body.body),
                    None => None,
                }
            } else {
                None
            };
            return sources.map(DeclarationSourceView::Body).ok_or_else(|| {
                ReadError::Query(ajika::Error::application(
                    "body declaration source is absent",
                ))
            });
        }
        SavedDeclarationSourceView::new(self.db, krate, source)
            .map(DeclarationSourceView::Saved)
            .map_err(ReadError::Query)
    }

    pub(crate) fn files(&self, krate: CrateRef) -> Result<UniqueVec<FileId>, ReadError> {
        Ok(self.owners(krate)?.iter().map(|owner| owner.file).collect())
    }

    pub(crate) fn body(
        &self,
        body: BodyRef,
    ) -> Result<Option<(BodyView<'_>, &BodyLocalItems)>, ReadError> {
        let Some(key) = body.saved_key() else {
            return Ok(None);
        };
        let Some(saved) = self.owner(key)? else {
            return Ok(None);
        };
        Ok(saved.body(body.body).zip(saved.body_local_items(body.body)))
    }

    pub(crate) fn prepared_body(
        &self,
        body: BodyRef,
    ) -> Result<Option<(&BodyData, &BodyLocalItems)>, ReadError> {
        let Some(key) = body.saved_key() else {
            return Ok(None);
        };
        if !self.targets.contains(&body.crate_ref) {
            return Ok(None);
        }
        let prepared = prepare_saved_body(self.db, key).map_err(ReadError::Query)?;
        let Some(prepared) = prepared.as_ref() else {
            return Ok(None);
        };
        let data = prepared.data.value_ref(self.db).map_err(ReadError::Query)?;
        Ok(data
            .bodies
            .get(body.body)
            .zip(data.local_items.get(body.body)))
    }

    pub(crate) fn prepared_bodies(
        &self,
        krate: CrateRef,
        file: FileId,
        selected: &dyn Fn(&BodyOwnerData) -> bool,
    ) -> Result<Vec<(BodyRef, &BodyData)>, ReadError> {
        let mut result = Vec::new();
        for owner in self
            .owners(krate)?
            .iter()
            .filter(|owner| owner.file == file && selected(owner))
        {
            let key = BodyKey {
                krate,
                owner: owner.owner,
            };
            let prepared = prepare_saved_body(self.db, key).map_err(ReadError::Query)?;
            let Some(prepared) = prepared.as_ref() else {
                continue;
            };
            let data = prepared.data.value_ref(self.db).map_err(ReadError::Query)?;
            result.extend(
                data.bodies
                    .iter_with_ids()
                    .map(|(slot, body)| (key.body(slot), body)),
            );
        }
        Ok(result)
    }

    pub(crate) fn prepared_owner(
        &self,
        key: BodyKey,
    ) -> Result<Option<Arc<PreparedBodyStore>>, ReadError> {
        if !self.targets.contains(&key.krate) {
            return Ok(None);
        }
        let prepared = prepare_saved_body(self.db, key).map_err(ReadError::Query)?;
        let Some(prepared) = prepared.as_ref() else {
            return Ok(None);
        };
        Ok(Some(
            prepared.data.value(self.db).map_err(ReadError::Query)?,
        ))
    }

    pub(crate) fn facts(&self, key: BodyKey) -> Result<Arc<BodyFactsStore>, ReadError> {
        infer_saved_body(self.db, key).map_err(ReadError::Query)
    }

    pub(crate) fn owners(&self, krate: CrateRef) -> Result<&[BodyOwnerData], ReadError> {
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

    pub(crate) fn bodies(
        &self,
        krate: CrateRef,
        file: Option<FileId>,
        selected: &dyn Fn(&BodyOwnerData) -> bool,
    ) -> Result<Vec<(BodyRef, BodyView<'_>)>, ReadError> {
        let keys: Vec<_> = self
            .owners(krate)?
            .iter()
            .filter(|root| file.is_none_or(|file| root.file == file) && selected(root))
            .map(|root| BodyKey {
                krate,
                owner: root.owner,
            })
            .collect();
        // Selection happens before inference. Ajika owns the batch so parallel owners retain
        // their individual dependency frames and share any candidate queries they demand.
        let profile = ProfileThreadContext::capture();
        self.db
            .parallel_map(&keys, |db, &key| {
                let _profile = profile.enter();
                saved_body(db, key).map(|_| ())
            })
            .map_err(ReadError::Query)?;
        let mut bodies = Vec::new();
        for key in keys {
            if let Some(owner) = self.owner(key)? {
                bodies.extend(owner.body_views().map(|(id, view)| (key.body(id), view)));
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
