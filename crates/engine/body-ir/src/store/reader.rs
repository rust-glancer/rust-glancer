use std::{collections::HashSet, fmt, sync::Arc, vec::IntoIter};

use rg_def_map::{DefMap, ItemSource, ItemSourceKind};
use rg_ir_model::{
    BodyKey, BodyRef, ConstOrStaticRef, CrateRef, DefMapRef, EvaluatedConstValue, FileId,
    ItemStoreRef, LocalDefRef, SemanticItemRef, SourceId, SourceLocation,
};
use rg_item_tree::SourceLocations;
use rg_profile::ProfileThreadContext;
use rg_semantic_ir::{
    AssociatedItemStore, BodyOwnerData, ItemSourceView, SemanticItemSource, SemanticItemView,
    SourceSelection, TargetDeclarationReader, body_owners,
};
use rg_source::{QueryContext, ReadError};
use rg_std::UniqueVec;

use crate::{
    BodyLocalItems, BodyOwner, BodySourceStore, BodyStore, BodyView, PreparedBodyStore, body_store,
    infer_body, prepare_body,
};

/// Owner metadata identifies bodies without lowering or inferring them. Enumerating methods can
/// still open their trait/impl member stores, so source requests narrow owners first by file,
/// cursor, or range; a known `BodyKey` goes straight to its owner.
#[derive(Clone)]
pub struct BodyReader<'db> {
    db: &'db QueryContext<'db>,
    targets: Arc<HashSet<CrateRef>>,
}

impl<'db> BodyReader<'db> {
    pub fn new(db: &'db QueryContext<'db>, declarations: &TargetDeclarationReader<'db>) -> Self {
        Self {
            db,
            targets: Arc::new(declarations.targets().collect()),
        }
    }

    fn owner(&self, key: BodyKey) -> Result<Option<&'db BodyStore>, ReadError> {
        if !self.targets.contains(&key.krate) {
            return Ok(None);
        }
        Ok(body_store(self.db, key).map_err(ReadError::Query)?.as_ref())
    }

    pub fn declaration_source(
        &self,
        krate: CrateRef,
        source: ItemSource,
    ) -> Result<DeclarationSourceView<'_>, ReadError> {
        if let ItemSourceKind::Body(source) = source.kind {
            // Resolving an occurrence only needs the location output. Keep this separate from
            // local declaration reads, which must observe prepared meaning alone.
            let sources = if let Some(key) = source.body.key()
                && self.targets.contains(&source.body.crate_ref)
            {
                let prepared = prepare_body(self.db, key).map_err(ReadError::Query)?;
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
        ItemSourceView::new(self.db, krate, source)
            .map(DeclarationSourceView::Item)
            .map_err(ReadError::Query)
    }

    /// Find source files from owner metadata without preparing their bodies.
    pub fn body_files(&self, krate: CrateRef) -> Result<Vec<FileId>, ReadError> {
        Ok(self
            .owners(krate, SourceSelection::All)?
            .map(|owner| owner.file)
            .collect::<UniqueVec<_>>()
            .into_vec())
    }

    pub fn body(&self, body: BodyRef) -> Result<Option<BodyView<'_>>, ReadError> {
        let Some(key) = body.key() else {
            return Ok(None);
        };
        Ok(self.owner(key)?.and_then(|owner| owner.body(body.body)))
    }

    fn body_local_items(&self, body: BodyRef) -> Result<Option<&BodyLocalItems>, ReadError> {
        let Some(key) = body.key() else {
            return Ok(None);
        };
        Ok(self
            .prepared_owner(key)?
            .and_then(|prepared| prepared.local_items.get(body.body)))
    }

    /// Semantic reads need prepared meaning, without acquiring locations or inferred facts.
    fn prepared_owner(&self, key: BodyKey) -> Result<Option<&PreparedBodyStore>, ReadError> {
        if !self.targets.contains(&key.krate) {
            return Ok(None);
        }
        let prepared = prepare_body(self.db, key).map_err(ReadError::Query)?;
        let Some(prepared) = prepared.as_ref() else {
            return Ok(None);
        };
        Ok(Some(
            prepared.data.value_ref(self.db).map_err(ReadError::Query)?,
        ))
    }

    pub fn owners(
        &self,
        krate: CrateRef,
        selection: SourceSelection,
    ) -> Result<IntoIter<BodyOwnerData>, ReadError> {
        if !self.targets.contains(&krate) {
            return Ok(Vec::new().into_iter());
        }
        Ok(body_owners(self.db, krate, selection)
            .map_err(ReadError::Query)?
            .into_iter())
    }

    pub fn bodies_matching(
        &self,
        krate: CrateRef,
        selection: SourceSelection,
        selected: &dyn Fn(&BodyOwnerData) -> bool,
    ) -> Result<Vec<(BodyRef, BodyView<'_>)>, ReadError> {
        let keys: Vec<_> = self
            .owners(krate, selection)?
            .filter(|root| selected(root))
            .map(|root| BodyKey {
                krate,
                owner: root.owner,
            })
            .collect();
        self.acquire_bodies(keys)
    }

    fn acquire_bodies(
        &self,
        keys: Vec<BodyKey>,
    ) -> Result<Vec<(BodyRef, BodyView<'_>)>, ReadError> {
        // Selection happens before inference. Ajika owns the batch so parallel owners retain
        // their individual dependency frames and share any candidate queries they demand.
        let profile = ProfileThreadContext::capture();
        self.db
            .parallel_map(&keys, |db, &key| {
                let _profile = profile.enter();
                body_store(db, key).map(|_| ())
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

    pub fn declaration_location(
        &self,
        krate: CrateRef,
        source: ItemSource,
        occurrence: SourceId,
    ) -> Result<SourceLocation, ReadError> {
        self.declaration_source(krate, source)?
            .location(occurrence)
            .ok_or_else(|| {
                ReadError::Query(ajika::Error::application(
                    "declaration occurrence is absent",
                ))
            })
    }

    pub fn bodies(
        &self,
        krate: CrateRef,
        selection: SourceSelection,
    ) -> Result<Vec<(BodyRef, BodyView<'_>)>, ReadError> {
        self.bodies_matching(krate, selection, &|_| true)
    }

    /// Speculative completion syntax may end before the cursor. Its declaration start can still
    /// select the ordinary owner; the speculative syntax itself never enters the semantic graph.
    pub fn bodies_at(
        &self,
        krate: CrateRef,
        file: FileId,
        offset: u32,
        owner_start: Option<u32>,
    ) -> Result<Vec<(BodyRef, BodyView<'_>)>, ReadError> {
        let Some(owner_start) = owner_start else {
            return self.bodies(krate, SourceSelection::at(file, offset));
        };
        let selection = SourceSelection::at(file, owner_start);
        let mut selected = Vec::new();
        for owner in self.owners(krate, selection)? {
            let span = self
                .declaration_location(krate, owner.source, owner.location)?
                .span;
            if span.touches(offset) || owner_start == span.start {
                selected.push(BodyKey {
                    krate,
                    owner: owner.owner,
                });
            }
        }
        self.acquire_bodies(selected)
    }

    /// Read the initializer's computed value without loading source maps.
    pub fn initializer_value(
        &self,
        item: ConstOrStaticRef,
    ) -> Result<Option<EvaluatedConstValue>, ReadError> {
        let owner = BodyOwner::from(item);
        let key = match item.origin() {
            DefMapRef::Crate(krate) => BodyKey {
                krate,
                owner: owner.into(),
            },
            DefMapRef::Body(body) => {
                let Some(key) = body.key() else {
                    return Ok(None);
                };
                key
            }
        };
        let Some(prepared) = self.prepared_owner(key)? else {
            return Ok(None);
        };
        let Some((slot, _)) = prepared
            .bodies
            .iter_with_ids()
            .find(|(_, body)| body.owner() == owner)
        else {
            return Ok(None);
        };
        let facts = infer_body(self.db, key).map_err(ReadError::Query)?;
        Ok(facts
            .bodies
            .get(slot)
            .and_then(|body| body.initializer_value()))
    }

    /// Return the body-local DefMap without exposing the containing storage object.
    pub fn body_def_map(&self, body_ref: BodyRef) -> Result<Option<&DefMap>, ReadError> {
        Ok(self
            .body_local_items(body_ref)?
            .map(BodyLocalItems::def_map))
    }
}

impl fmt::Debug for BodyReader<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BodyReader")
            .field("targets", &self.targets.len())
            .finish_non_exhaustive()
    }
}

impl<'a> SemanticItemSource<'a> for &'a BodyReader<'_> {
    fn associated_items(
        &self,
        store: ItemStoreRef,
    ) -> Result<Option<&'a AssociatedItemStore>, ReadError> {
        let DefMapRef::Body(body) = store.origin else {
            return Ok(None);
        };
        Ok(self
            .body_local_items(body)?
            .and_then(|items| items.associated_items(store)))
    }

    fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'a>>, ReadError> {
        let DefMapRef::Body(body) = item.origin() else {
            return Ok(None);
        };
        Ok(self
            .body_local_items(body)?
            .and_then(|items| items.item(item)))
    }

    fn item_for_local_def(
        &self,
        definition: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, ReadError> {
        let DefMapRef::Body(body) = definition.origin else {
            return Ok(None);
        };
        Ok(self
            .body_local_items(body)?
            .map(BodyLocalItems::item_store)
            .and_then(|items| {
                items
                    .item_for_local_def(definition.local_def)
                    .map(|item| item.semantic_ref(definition.origin))
            }))
    }

    fn scope_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'a>>, ReadError> {
        let DefMapRef::Body(body) = origin else {
            return Ok(Vec::new());
        };
        Ok(self
            .body_local_items(body)?
            .map(BodyLocalItems::item_store)
            .map(|items| items.scope_items().collect())
            .unwrap_or_default())
    }

    fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'a>>, ReadError> {
        let DefMapRef::Body(body) = origin else {
            return Ok(Vec::new());
        };
        Ok(self
            .body_local_items(body)?
            .map(|items| items.all_items().collect())
            .unwrap_or_default())
    }

    fn source_items(
        &self,
        origin: DefMapRef,
        selection: SourceSelection,
    ) -> Result<Vec<SemanticItemView<'a>>, ReadError> {
        // Local stores are already prepared; the scan checks exact ranges.
        Ok(self
            .all_items(origin)?
            .into_iter()
            .filter(|item| {
                selection
                    .file()
                    .is_none_or(|file| item.source().file_id == file)
            })
            .collect())
    }
}

/// A declaration's meaning carries an [`ItemSource`]; this view joins it to the selected snapshot.
///
/// File and expansion sources need the tables opened by [`ItemSourceView`].
/// Body sources already contain resolved locations, so this view borrows their [`BodySourceStore`]
/// instead.
///
/// Both forms expose [`SourceLocations`], keeping the navigation/editability distinction the same
/// for source scanners regardless of where the declaration was lowered.
pub enum DeclarationSourceView<'a> {
    Item(ItemSourceView<'a>),
    Body(&'a BodySourceStore),
}

impl<'a> DeclarationSourceView<'a> {
    pub fn initializer_preview(&self, occurrence: SourceId) -> Option<&'a str> {
        match self {
            Self::Item(source) => source.initializer_preview(occurrence),
            Self::Body(source) => source.initializer_preview(occurrence),
        }
    }
}

impl SourceLocations for DeclarationSourceView<'_> {
    fn location(&self, occurrence: SourceId) -> Option<SourceLocation> {
        match self {
            Self::Item(source) => source.location(occurrence),
            Self::Body(source) => source.location(occurrence),
        }
    }
}
