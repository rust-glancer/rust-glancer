//! Body reads that combine saved owner results with request-local declarations.

mod saved;

use std::sync::Arc;

use rg_def_map::{DefMap, ItemSource, ItemSourceKind};
use rg_ir_model::{
    BodyKey, BodyRef, ConstOrStaticRef, CrateRef, DefMapRef, EvaluatedConstValue, FileId, ImplRef,
    LocalDefRef, ModuleRef, SemanticItemRef, SourceId, SourceLocation, Span,
};
use rg_item_tree::SourceLocations;
use rg_semantic_ir::{
    BodyOwnerData, SavedDeclarationSourceView, SemanticItemSource, SemanticItemView,
    TargetDeclarationReader,
};
use rg_source::{QueryContext, ReadError};

use self::saved::SavedBodyReader;
use crate::{
    BodyData, BodyFactsStore, BodyLocalItems, BodySourceStore, BodyView, CurrentSourceStore,
    PreparedBodyStore,
};

/// One operation's saved body reads and its optional current-source overlay.
#[derive(Debug, Clone)]
pub struct BodyReader<'db> {
    saved: Arc<SavedBodyReader<'db>>,
    current: Arc<CurrentSourceStore>,
}

impl<'db> BodyReader<'db> {
    /// Open the table that owns a declaration's occurrence IDs. A rebuilt body or current impl
    /// header supplies its own table; other declarations use their saved source. The source
    /// identity decides this, rather than whether the same file happens to be open in the editor.
    pub fn declaration_source(
        &self,
        krate: CrateRef,
        source: ItemSource,
    ) -> Result<DeclarationSourceView<'_>, ReadError> {
        if let ItemSourceKind::Body(source) = source.kind
            && let Some(sources) = self.current.sources(source.body)
        {
            return Ok(DeclarationSourceView::Body(sources));
        }
        self.saved.declaration_source(krate, source)
    }

    /// Resolve one required occurrence. When walking several occurrences from the same source,
    /// open [`Self::declaration_source`] once and use its [`SourceLocations`] interface instead.
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

    pub fn new(db: &'db QueryContext<'db>, declarations: &TargetDeclarationReader<'db>) -> Self {
        Self {
            saved: Arc::new(SavedBodyReader::new(db, declarations.targets())),
            current: Arc::new(CurrentSourceStore::default()),
        }
    }

    /// Attach the frozen bodies and declaration contexts prepared for this request.
    pub fn with_current_source(mut self, current: CurrentSourceStore) -> Self {
        self.current = Arc::new(current);
        self
    }

    /// Return whether this body was rebuilt from the request's current source.
    pub fn is_current_body(&self, body_ref: BodyRef) -> bool {
        self.current.contains_body(body_ref)
    }

    pub fn is_current_origin(&self, origin: DefMapRef) -> bool {
        self.current.contains_origin(origin)
    }

    pub fn current_signature_origins(
        &self,
        crate_ref: CrateRef,
        file: FileId,
    ) -> impl Iterator<Item = DefMapRef> + '_ {
        self.current.signature_origins(crate_ref, file)
    }

    /// Header-only modules borrow saved names; body-owned modules retain lexical parent lookup.
    pub fn signature_lookup_module(&self, module: ModuleRef) -> Result<ModuleRef, ReadError> {
        if let Some(fallback) = self.current.declaration_fallback(module) {
            return Ok(fallback);
        }
        if let DefMapRef::Body(body_ref) = module.origin {
            // Signature lookup needs the lexical context, not expression facts. A local
            // declaration read must not force inference of the body that declares it.
            let body = if let Some(current) = self
                .current
                .bodies()
                .iter()
                .find(|body| body.body_ref() == body_ref)
            {
                Some(current.view().structure())
            } else {
                self.saved_prepared_body(body_ref)?.map(|(body, _)| body)
            };
            if let Some(body) = body
                && module == body.owner_module()
            {
                return Ok(body.fallback_module());
            }
        }
        Ok(module)
    }

    pub fn selected_current_impl(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        span: Span,
    ) -> Option<ImplRef> {
        self.current.selected_impl(crate_ref, file, span)
    }

    /// List source files with body owners before reading any body results.
    ///
    /// Whole-crate query work can visit these files separately, checking cancellation before each
    /// read. Current bodies contribute their own files.
    pub fn body_files(&self, crate_ref: CrateRef) -> Result<Vec<FileId>, ReadError> {
        let mut files = self.saved.files(crate_ref)?;
        files.extend(
            self.current
                .bodies()
                .iter()
                .filter(|body| body.body_ref().crate_ref == crate_ref)
                .map(|body| body.view().source().file_id),
        );
        Ok(files.into_vec())
    }

    /// Enumerate bodies from one file, or every body when `file` is absent.
    ///
    /// File-scoped callers acquire only owners in the selected file.
    pub fn bodies(
        &self,
        crate_ref: CrateRef,
        file: Option<FileId>,
    ) -> Result<Vec<(BodyRef, BodyView<'_>)>, ReadError> {
        self.bodies_matching(crate_ref, file, &|_| true)
    }

    /// Discover saved owners visible in this view before computing them. A changed current file
    /// masks its saved owners; its prepared bodies are added separately by `bodies_matching`.
    pub fn owners(
        &self,
        crate_ref: CrateRef,
    ) -> Result<impl Iterator<Item = &BodyOwnerData> + Clone, ReadError> {
        Ok(self
            .saved
            .owners(crate_ref)?
            .iter()
            .filter(move |owner| !self.current.masks_file(crate_ref, owner.file)))
    }

    /// A cursor selects enclosing outer owners. Speculative syntax may end before the cursor;
    /// its known declaration start still selects that one owner and its nested work.
    pub fn bodies_at(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        offset: u32,
        owner_start: Option<u32>,
    ) -> Result<Vec<(BodyRef, BodyView<'_>)>, ReadError> {
        let mut selected = Vec::new();
        for owner in self.owners(crate_ref)?.filter(|owner| owner.file == file) {
            let span = self
                .declaration_location(crate_ref, owner.source, owner.location)?
                .span;
            if span.touches(offset) || owner_start == Some(span.start) {
                selected.push(owner.owner);
            }
        }
        self.bodies_matching(crate_ref, Some(file), &|owner| {
            selected.contains(&owner.owner)
        })
    }

    pub fn bodies_matching(
        &self,
        crate_ref: CrateRef,
        file: Option<FileId>,
        selected: &dyn Fn(&BodyOwnerData) -> bool,
    ) -> Result<Vec<(BodyRef, BodyView<'_>)>, ReadError> {
        let mut saved = self.saved.bodies(crate_ref, file, &|owner| {
            !self.current.masks_file(crate_ref, owner.file) && selected(owner)
        })?;
        saved.retain(|(body_ref, body)| {
            !self.current.contains_body(*body_ref)
                && !self.current.masks_file(crate_ref, body.source().file_id)
        });

        // Add rebuilt bodies after removing their saved identities and any saved bodies masked by
        // different editor text. Their expressions and locals come from the current source.
        for current in self.current.bodies().iter().filter(|body| {
            body.body_ref().crate_ref == crate_ref
                && file.is_none_or(|file| body.view().source().file_id == file)
        }) {
            saved.push((current.body_ref(), current.view()));
        }

        Ok(saved)
    }

    /// Select a body by its saved owner and local ID, or by its request-local identity.
    pub fn body(&self, body_ref: BodyRef) -> Result<Option<BodyView<'_>>, ReadError> {
        if let Some(body) = self
            .current
            .bodies()
            .iter()
            .find(|body| body.body_ref() == body_ref)
        {
            return Ok(Some(body.view()));
        }
        Ok(self.saved_body(body_ref)?.map(|(body, _)| body))
    }

    fn body_local_items(&self, body_ref: BodyRef) -> Result<Option<&BodyLocalItems>, ReadError> {
        if let Some(items) = self.current.items(DefMapRef::Body(body_ref)) {
            return Ok(Some(items));
        }
        Ok(self.saved_prepared_body(body_ref)?.map(|(_, items)| items))
    }

    /// Read local meaning without acquiring source tables or inferred facts.
    fn saved_prepared_body(
        &self,
        body_ref: BodyRef,
    ) -> Result<Option<(&BodyData, &BodyLocalItems)>, ReadError> {
        // Only a changed editor file needs masking. Its outer-owner metadata also identifies
        // the file of nested bodies, so checking visibility never needs occurrence locations.
        if let Some((krate, file)) = self.current.masked_file
            && krate == body_ref.crate_ref
            && let Some(owner) = body_ref.root.owner()
            && self
                .saved
                .owners(krate)?
                .iter()
                .any(|data| data.owner == owner && data.file == file)
        {
            return Ok(None);
        }
        self.saved.prepared_body(body_ref)
    }

    pub(crate) fn saved_prepared_bodies(
        &self,
        krate: CrateRef,
        file: FileId,
        selected: &dyn Fn(&BodyOwnerData) -> bool,
    ) -> Result<Vec<(BodyRef, &BodyData)>, ReadError> {
        self.saved.prepared_bodies(krate, file, selected)
    }

    pub(crate) fn saved_prepared_owner(
        &self,
        key: BodyKey,
    ) -> Result<Option<Arc<PreparedBodyStore>>, ReadError> {
        self.saved.prepared_owner(key)
    }

    pub(crate) fn saved_facts(&self, key: BodyKey) -> Result<Arc<BodyFactsStore>, ReadError> {
        self.saved.facts(key)
    }

    fn saved_body(
        &self,
        body_ref: BodyRef,
    ) -> Result<Option<(BodyView<'_>, &BodyLocalItems)>, ReadError> {
        Ok(self.saved.body(body_ref)?.filter(|(body, _)| {
            !self
                .current
                .masks_file(body_ref.crate_ref, body.source().file_id)
        }))
    }

    pub fn saved_initializer_value(
        &self,
        item: ConstOrStaticRef,
    ) -> Result<Option<&EvaluatedConstValue>, ReadError> {
        self.saved.initializer(item)
    }

    /// Return the body-local DefMap without exposing the containing storage object.
    pub fn body_def_map(&self, body_ref: BodyRef) -> Result<Option<&DefMap>, ReadError> {
        Ok(self
            .body_local_items(body_ref)?
            .map(BodyLocalItems::def_map))
    }
}

impl<'a> SemanticItemSource<'a> for &'a BodyReader<'_> {
    fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'a>>, ReadError> {
        let DefMapRef::Body(body) = item.origin() else {
            return Ok(None);
        };
        Ok(self
            .body_local_items(body)?
            .map(BodyLocalItems::item_store)
            .and_then(|items| items.semantic_item_view(item)))
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

    fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'a>>, ReadError> {
        let DefMapRef::Body(body) = origin else {
            return Ok(Vec::new());
        };
        Ok(self
            .body_local_items(body)?
            .map(BodyLocalItems::item_store)
            .map(|items| items.semantic_items().collect())
            .unwrap_or_default())
    }
}

/// A declaration's meaning carries an [`ItemSource`]; this view joins it to the selected snapshot.
///
/// Saved file and expansion sources need the tables opened by [`SavedDeclarationSourceView`].
/// Body sources already contain resolved locations, so this view borrows their [`BodySourceStore`]
/// instead. The body can be saved or rebuilt for this request; [`BodyReader`] chooses it first.
///
/// Both forms expose [`SourceLocations`], keeping the navigation/editability distinction the same
/// for source scanners regardless of where the declaration was lowered.
pub enum DeclarationSourceView<'a> {
    Saved(SavedDeclarationSourceView<'a>),
    Body(&'a BodySourceStore),
}

impl SourceLocations for DeclarationSourceView<'_> {
    fn location(&self, occurrence: SourceId) -> Option<SourceLocation> {
        match self {
            Self::Saved(source) => source.location(occurrence),
            Self::Body(source) => source.location(occurrence),
        }
    }
}
