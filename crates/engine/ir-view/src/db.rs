//! Shared read handle for indexed-data views.

use std::sync::Arc;

use anyhow::Context as _;
use rg_body_ir::{BodyReader, CurrentSourceBuilder, CurrentSourceStore, DeclarationSourceView};
use rg_def_map::{DefMap, DefMapSource, DefinitionReader, ItemSource};
use rg_ir_model::{
    BodyRef, CrateRef, DefMapRef, FileId, ImplRef, ItemStoreRef, LocalDefRef, ModuleRef,
    PackageSlot, Path, SemanticItemRef, SourceId, Span,
};
use rg_parse::{CurrentSource, Package, ParseDb, ParsedFile, saved_syntax};
use rg_semantic_ir::{
    AssociatedItemStore, ItemLookupQuery, ItemReader, SemanticItemSource, SemanticItemView,
    TypePathContext, TypePathResolution,
};
use rg_source::{QueryContext, ReadError};
use rg_std::{Cancelable, CancellationToken, Cancelled, UniqueVec};
use rg_syntax::{Parse, SourceFile};
use rg_text::RustEdition;
use rg_ty::{
    lookup::ItemPathQuery,
    lowering::{TypeLoweringAnchor, TypePathResolver},
};

/// Read-only database handle used by all indexed-data views.
///
/// Saved readers and prepared current declarations stay frozen for the request. Demanding lookup
/// candidates never changes which declarations or bodies are visible through this handle.
#[derive(Debug, Clone)]
pub struct IndexedViewDb<'db> {
    pub(crate) def_map: DefinitionReader<'db>,
    pub(crate) semantic_ir: ItemReader<'db>,
    pub(crate) body_ir: BodyReader<'db>,
    parse: &'db ParseDb,
    queries: &'db QueryContext<'db>,
    cancellation: CancellationToken,
}

impl Cancelable for IndexedViewDb<'_> {
    fn check_cancelled(&self, checkpoint: &'static str) -> Result<(), Cancelled> {
        Cancelable::check_cancelled(&self.cancellation, checkpoint)
    }
}

impl<'db> IndexedViewDb<'db> {
    /// Open the source owner before interpreting any occurrence IDs.
    pub(crate) fn declaration_source(
        &self,
        origin: DefMapRef,
        source: ItemSource,
    ) -> Result<DeclarationSourceView<'_>, ReadError> {
        self.body_ir
            .declaration_source(origin.origin_crate(), source)
    }

    pub(crate) fn source_span(
        &self,
        origin: DefMapRef,
        source: ItemSource,
        occurrence: SourceId,
    ) -> Result<Span, ReadError> {
        self.body_ir
            .declaration_location(origin.origin_crate(), source, occurrence)
            .map(|location| location.span)
    }

    pub fn new(
        queries: &'db QueryContext<'db>,
        def_map: DefinitionReader<'db>,
        semantic_ir: ItemReader<'db>,
        body_ir: BodyReader<'db>,
        parse: &'db ParseDb,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            body_ir,
            parse,
            queries,
            cancellation,
        }
    }

    pub(crate) fn parsed_file(&self, krate: CrateRef, file: FileId) -> Option<ParsedFile<'db>> {
        self.parse.package(krate.package.0)?.parsed_file(file)
    }

    /// Share the captured syntax with body providers in this operation.
    pub(crate) fn saved_syntax(
        &self,
        krate: CrateRef,
        file: FileId,
    ) -> anyhow::Result<Arc<Parse<SourceFile>>> {
        Ok(saved_syntax(
            self.queries,
            file,
            self.crate_edition(krate)?,
        )?)
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    /// Lend the saved readers used by the eventual request queries.
    pub fn current_source_builder<'request>(
        &'request self,
        source: &'request CurrentSource,
    ) -> CurrentSourceBuilder<'request, 'db> {
        CurrentSourceBuilder::new(
            &self.def_map,
            &self.semantic_ir,
            &self.body_ir,
            source,
            self.cancellation.clone(),
        )
    }

    pub fn with_current_source(mut self, current: CurrentSourceStore) -> Self {
        self.body_ir = self.body_ir.with_current_source(current);
        self
    }

    /// Only the declaration contexts selected for ordinary source scanning are exposed here.
    /// Complete impls prepared alongside body contexts remain private to member queries.
    pub(crate) fn current_signature_origins(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
    ) -> impl Iterator<Item = DefMapRef> + '_ {
        self.body_ir.current_signature_origins(crate_ref, file_id)
    }

    /// Keep temporary generic and impl identities while looking up paths in their containing scope.
    pub(crate) fn current_signature_context(
        &self,
        mut context: TypePathContext,
    ) -> Result<TypePathContext, ReadError> {
        context.module = self.body_ir.signature_lookup_module(context.module)?;
        Ok(context)
    }

    pub fn selected_current_impl(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        span: Span,
    ) -> Option<ImplRef> {
        self.body_ir.selected_current_impl(crate_ref, file, span)
    }

    pub(crate) fn def_map_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&DefMap>, ReadError> {
        match origin {
            DefMapRef::Crate(crate_ref) => self.def_map.def_map(crate_ref),
            DefMapRef::Body(body_ref) => self.body_ir.body_def_map(body_ref),
        }
    }

    /// Return whether a body identity belongs to request-local current Body IR.
    pub fn is_current_body(&self, body_ref: BodyRef) -> bool {
        self.body_ir.is_current_body(body_ref)
    }

    /// Return whether a semantic origin was created from this request's editor source.
    pub fn is_current_origin(&self, origin: DefMapRef) -> bool {
        self.body_ir.is_current_origin(origin)
    }

    /// Assemble the declarations visible from one crate without copying dependency indexes.
    ///
    /// Only small visibility and language-item facts are acquired here. Individual candidate
    /// questions demand their own answers through the same operation.
    pub(crate) fn item_lookup_query(
        &self,
        use_site: CrateRef,
    ) -> anyhow::Result<ItemLookupQuery<'_>> {
        ItemLookupQuery::build_from(self.semantic_ir, use_site, &self.cancellation)
            .context("assemble visible semantic item indexes")
    }

    /// Returns the edition whose syntax rules apply at a crate_ref use site.
    pub fn crate_edition(&self, crate_ref: CrateRef) -> anyhow::Result<RustEdition> {
        Ok(self
            .parse
            .package(crate_ref.package.0)
            .context("crate has no parsed package")?
            .edition())
    }

    pub(crate) fn package_name(&self, package: PackageSlot) -> Option<&str> {
        self.parse.package(package.0).map(Package::package_name)
    }

    /// Returns the edition whose syntax rules apply to declarations owned by this origin.
    pub fn origin_edition(&self, origin: DefMapRef) -> anyhow::Result<RustEdition> {
        self.crate_edition(origin.origin_crate())
    }
}

/// Resolve request-local signature paths from their real containing module.
///
/// Generic and impl identities remain attached to the temporary declaration. Only the module
/// lookup starting point is replaced; after that, the ordinary indexed path query owns all name
/// resolution rules.
impl TypePathResolver for IndexedViewDb<'_> {
    fn resolve_type_path(
        &self,
        anchor: TypeLoweringAnchor,
        path: &Path,
    ) -> Result<TypePathResolution, ReadError> {
        let TypeLoweringAnchor::Context(context) = anchor else {
            return Ok(TypePathResolution::Unknown);
        };
        let context = self.current_signature_context(context)?;
        ItemPathQuery::new(self, self).resolve_type_path(context, path)
    }
}

impl<'a, 'db> SemanticItemSource<'a> for &'a IndexedViewDb<'db> {
    fn associated_items(
        &self,
        store: ItemStoreRef,
    ) -> Result<Option<&'a AssociatedItemStore>, ReadError> {
        match store.origin {
            DefMapRef::Crate(_) => self.semantic_ir.associated_items(store),
            DefMapRef::Body(_) => (&self.body_ir).associated_items(store),
        }
    }

    fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'a>>, ReadError> {
        match item.origin() {
            DefMapRef::Crate(_) => self.semantic_ir.item(item),
            DefMapRef::Body(_) => (&self.body_ir).item(item),
        }
    }

    fn item_for_local_def(
        &self,
        definition: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, ReadError> {
        match definition.origin {
            DefMapRef::Crate(_) => self.semantic_ir.item_for_local_def(definition),
            DefMapRef::Body(_) => (&self.body_ir).item_for_local_def(definition),
        }
    }

    fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'a>>, ReadError> {
        match origin {
            DefMapRef::Crate(_) => self.semantic_ir.all_items(origin),
            DefMapRef::Body(_) => (&self.body_ir).all_items(origin),
        }
    }
}

impl DefMapSource for &IndexedViewDb<'_> {
    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, ReadError> {
        match origin {
            DefMapRef::Crate(crate_ref) => self.def_map.def_map(crate_ref),
            DefMapRef::Body(body_ref) => self.body_ir.body_def_map(body_ref),
        }
    }

    fn crate_is_proc_macro(&self, crate_ref: CrateRef) -> Result<bool, ReadError> {
        self.def_map.crate_is_proc_macro(crate_ref)
    }

    fn target_pointer_width(&self, crate_ref: CrateRef) -> Result<Option<u32>, ReadError> {
        self.def_map.target_pointer_width(crate_ref)
    }

    fn extern_root(&self, crate_ref: CrateRef, name: &str) -> Result<Option<ModuleRef>, ReadError> {
        self.def_map.extern_root(crate_ref, name)
    }

    fn extern_roots(&self, crate_ref: CrateRef) -> Result<Vec<(String, ModuleRef)>, ReadError> {
        self.def_map.extern_roots(crate_ref)
    }

    fn prelude_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, ReadError> {
        self.def_map.prelude_module(crate_ref)
    }

    fn item_lookup_dependencies(
        &self,
        crate_ref: CrateRef,
    ) -> Result<UniqueVec<CrateRef>, ReadError> {
        self.def_map.item_lookup_dependencies(crate_ref)
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, ReadError> {
        self.def_map.root_module(crate_ref)
    }
}
