use std::{collections::HashMap, convert::Infallible, fmt::Write as _};

use expect_test::Expect;
use rg_def_map::{
    DefMap, DefMapBuilder, DefMapSource, GeneratedItemRef, GeneratedSourceId, ItemSource,
    ItemSourceKind, LocalDefData, LocalDefKind, ModuleData, ModuleOrigin, ModuleScopeBuilder,
    Namespace, NamespaceSet, ScopeBinding, ScopeBindingProvenance, Visibility,
};
use rg_ir_model::{
    AssocItemId, CrateRef, DefId, DefMapRef, FileId, FloatTy, FunctionId, FunctionRef,
    GenericParamRef, ImplId, ItemId, ItemOwner, LocalDefId, LocalDefRef, LocalImplId, LocalImplRef,
    ModuleId, ModuleRef, PackageSlot, SignedIntTy, Span, StructId, TraitDefRef, TraitId,
    TypeAliasId, TypeAliasRef, TypeDefId, TypeDefRef, UnsignedIntTy,
};
use rg_item_tree::{
    FieldList, FunctionItem, FunctionQualifiers, GenericArg as ItemGenericArg, GenericParams,
    ItemTreeId, TraitBoundModifier, TypeAliasItem, TypeBound, TypeOrConstParamData, TypeParamData,
    TypePath, TypePathAnchor, TypePathSegment, TypeRef, VisibilityLevel, WherePredicate,
};
use rg_semantic_ir::{
    CrateItemQuery, FunctionData, FunctionSignature, GenericParamSource, GenericsQuery, ImplData,
    ItemLookupIndex, ItemLookupIndexSource, ItemLookupQuery, ItemStore, ItemStoreBuilder,
    ItemStoreSource, StructData, TraitData, TypeAliasData, TypeAliasSignature,
};
use rg_std::{ExpectedUnique, UniqueVec};
use rg_text::Name;

use crate::{
    AdtTy, AliasTy, GenericArg, OpaqueTy, PrimitiveTy, Ty, TyContext,
    lookup::ItemPathQuery,
    lowering::TypeLoweringQuery,
    signature::SemanticSignatureQuery,
    solver::{self, InferenceTable, Outcome},
};

pub(super) struct TraitSelectionFixture {
    def_map: DefMap,
    pub(super) store: ItemStore,
    pub(super) target: CrateRef,
    lookup_index: ItemLookupIndex,
    dependencies: Vec<TraitSelectionDependency>,
    type_names: HashMap<TypeDefRef, String>,
    trait_names: HashMap<TraitDefRef, String>,
    type_refs_by_name: HashMap<String, TypeDefRef>,
    trait_refs_by_name: HashMap<String, TraitDefRef>,
}

impl TraitSelectionFixture {
    // Tests use a small declarative fixture language instead of full Rust source. That keeps the
    // unit tests close to the trait-selection data model while still making the setup readable in
    // snapshots and reviews.
    pub(super) fn new(source: &str) -> Self {
        TraitSelectionFixtureParser::new(source).parse()
    }

    fn dependency(&self, crate_ref: CrateRef) -> Option<&TraitSelectionDependency> {
        self.dependencies
            .iter()
            .find(|dependency| dependency.target == crate_ref)
    }

    pub(super) fn lookup_query(&self) -> ItemLookupQuery<'_> {
        ItemLookupQuery::build_from(
            &CrateItemQuery::new(self, self, self.target),
            &rg_std::CancellationToken::new(),
        )
        .expect("fixture lookup query should build")
    }

    pub(super) fn type_ref_by_name(&self, name: &str) -> Option<TypeDefRef> {
        self.type_refs_by_name.get(name).copied()
    }

    pub(super) fn trait_ref_by_name(&self, name: &str) -> Option<TraitDefRef> {
        self.trait_refs_by_name.get(name).copied()
    }

    pub(super) fn associated_ty_by_name(
        &self,
        trait_ref: TraitDefRef,
        name: &str,
    ) -> Option<TypeAliasRef> {
        let trait_data = self.store.trait_data(trait_ref.id)?;
        trait_data.items.iter().find_map(|item| {
            let AssocItemId::TypeAlias(id) = item else {
                return None;
            };
            let data = self.store.type_alias_data(*id)?;
            (data.name.as_str() == name).then_some(TypeAliasRef {
                origin: trait_ref.origin,
                id: *id,
            })
        })
    }
}

impl From<&str> for TraitSelectionFixture {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl DefMapSource for TraitSelectionFixture {
    type Error = Infallible;

    fn def_map_for_origin(&self, origin_ref: DefMapRef) -> Result<Option<&DefMap>, Self::Error> {
        if origin_ref == DefMapRef::Crate(self.target) {
            return Ok(Some(&self.def_map));
        }
        Ok(origin_ref
            .as_crate_ref()
            .and_then(|crate_ref| self.dependency(crate_ref))
            .map(|dependency| &dependency.def_map))
    }

    fn crate_is_proc_macro(&self, _crate_ref: CrateRef) -> Result<bool, Self::Error> {
        Ok(false)
    }

    fn extern_root(
        &self,
        _target: CrateRef,
        _name: &str,
    ) -> Result<Option<ModuleRef>, Self::Error> {
        Ok(None)
    }

    fn extern_roots(&self, _target: CrateRef) -> Result<Vec<(String, ModuleRef)>, Self::Error> {
        Ok(Vec::new())
    }

    fn prelude_module(&self, _target: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        Ok(None)
    }

    fn item_lookup_dependencies(
        &self,
        crate_ref: CrateRef,
    ) -> Result<UniqueVec<CrateRef>, Self::Error> {
        if crate_ref != self.target {
            return Ok(UniqueVec::new());
        }
        Ok(self
            .dependencies
            .iter()
            .map(|dependency| dependency.target)
            .collect())
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        let known = crate_ref == self.target || self.dependency(crate_ref).is_some();
        Ok(known.then_some(ModuleRef {
            origin: DefMapRef::Crate(crate_ref),
            module: ModuleId(0),
        }))
    }
}

struct TraitSelectionDependency {
    target: CrateRef,
    def_map: DefMap,
    store: ItemStore,
    lookup_index: ItemLookupIndex,
}

impl<'a> ItemStoreSource<'a> for &'a TraitSelectionFixture {
    type Error = Infallible;

    fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&'a ItemStore>, Self::Error> {
        if origin == DefMapRef::Crate(self.target) {
            return Ok(Some(&self.store));
        }
        Ok(origin
            .as_crate_ref()
            .and_then(|crate_ref| self.dependency(crate_ref))
            .map(|dependency| &dependency.store))
    }

    fn included_stores(&self) -> Result<Vec<&'a ItemStore>, Self::Error> {
        Ok(std::iter::once(&self.store)
            .chain(self.dependencies.iter().map(|dependency| &dependency.store))
            .collect())
    }
}

impl<'a> ItemLookupIndexSource<'a> for &'a TraitSelectionFixture {
    fn item_lookup_index(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Option<&'a ItemLookupIndex>, Self::Error> {
        if crate_ref == self.target {
            return Ok(Some(&self.lookup_index));
        }
        Ok(self
            .dependency(crate_ref)
            .map(|dependency| &dependency.lookup_index))
    }
}

pub(super) fn target() -> CrateRef {
    CrateRef {
        package: PackageSlot(0),
        crate_id: rg_ir_model::CrateId(0),
    }
}

pub(super) fn origin() -> DefMapRef {
    DefMapRef::Crate(target())
}

pub(super) fn module() -> ModuleRef {
    ModuleRef {
        origin: origin(),
        module: ModuleId(0),
    }
}

pub(super) fn type_def(index: usize) -> TypeDefRef {
    TypeDefRef {
        origin: origin(),
        id: TypeDefId::Struct(StructId(index)),
    }
}

pub(super) fn trait_ref(index: usize) -> TraitDefRef {
    TraitDefRef {
        origin: origin(),
        id: TraitId(index),
    }
}

fn local_def(index: usize) -> LocalDefRef {
    LocalDefRef {
        origin: origin(),
        local_def: LocalDefId(index),
    }
}

fn dummy_source() -> ItemSource {
    ItemSource {
        // Source coordinates are irrelevant for trait-selection tests; generated fixtures only
        // need stable dummy identities.
        file_id: FileId(0),
        kind: ItemSourceKind::Generated(GeneratedItemRef {
            source: GeneratedSourceId(0),
            item: ItemTreeId(0),
        }),
    }
}

fn fixture_span() -> Span {
    Span { start: 0, end: 0 }
}

pub(super) fn path_ty(path: &str, args: Vec<ItemGenericArg>) -> TypeRef {
    let span = fixture_span();
    let mut segments = path
        .split("::")
        .map(|name| TypePathSegment {
            name: Name::new(name),
            args: Vec::new(),
            span,
        })
        .collect::<Vec<_>>();
    let final_segment = segments
        .last_mut()
        .expect("fixture path should have at least one segment");
    final_segment.args = args;

    TypeRef::Path(TypePath {
        source_span: span,
        absolute: false,
        anchor: None,
        segments,
    })
}

pub(super) fn qualified_assoc_ty(self_ty: TypeRef, trait_ty: TypeRef, assoc_name: &str) -> TypeRef {
    let span = fixture_span();
    TypeRef::Path(TypePath {
        source_span: span,
        absolute: false,
        anchor: Some(TypePathAnchor::QualifiedTrait {
            self_ty: Box::new(self_ty),
            trait_ty: Box::new(trait_ty),
        }),
        segments: vec![TypePathSegment {
            name: Name::new(assoc_name),
            args: Vec::new(),
            span,
        }],
    })
}

pub(super) fn type_arg(ty: TypeRef) -> ItemGenericArg {
    ItemGenericArg::Type(ty)
}

pub(super) fn type_param(name: &str) -> TypeParamData {
    TypeParamData {
        name: Name::new(name),
        bounds: Vec::new(),
        default: None,
    }
}

pub(super) fn type_param_with_bounds(name: &str, bounds: Vec<TypeBound>) -> TypeParamData {
    TypeParamData {
        name: Name::new(name),
        bounds,
        default: None,
    }
}

pub(super) fn generics(types: Vec<TypeParamData>) -> GenericParams {
    GenericParams {
        type_or_consts: types.into_iter().map(TypeOrConstParamData::Type).collect(),
        ..GenericParams::default()
    }
}

pub(super) fn trait_data(index: usize, name: &str, generics: GenericParams) -> TraitData {
    trait_data_with_items(index, name, generics, Vec::new())
}

pub(super) fn trait_data_with_items(
    index: usize,
    name: &str,
    generics: GenericParams,
    items: Vec<AssocItemId>,
) -> TraitData {
    TraitData {
        local_def: local_def(index),
        source: dummy_source(),
        owner: module(),
        name: Name::new(name),
        visibility: VisibilityLevel::Public,
        docs: None,
        generics,
        super_traits: Vec::new(),
        items,
        is_unsafe: false,
    }
}

pub(super) fn struct_data(index: usize, name: &str, generics: GenericParams) -> StructData {
    StructData {
        local_def: local_def(100 + index),
        source: dummy_source(),
        owner: module(),
        name: Name::new(name),
        visibility: VisibilityLevel::Public,
        docs: None,
        generics,
        fields: FieldList::Unit,
    }
}

pub(super) fn type_alias_data(
    name: &str,
    owner: ItemOwner,
    bounds: Vec<TypeBound>,
    aliased_ty: Option<TypeRef>,
) -> TypeAliasData {
    TypeAliasData {
        local_def: None,
        source: dummy_source(),
        span: Span { start: 0, end: 0 },
        name_span: None,
        owner,
        name: Name::new(name),
        visibility: VisibilityLevel::Public,
        docs: None,
        signature: TypeAliasSignature::from_item(&TypeAliasItem {
            generics: GenericParams::default(),
            bounds,
            aliased_ty,
        }),
    }
}

#[derive(Clone, Copy)]
enum FixtureSection {
    Traits,
    Structs,
    Impls,
    Functions,
    TypeAliases,
}

struct TraitSelectionFixtureParser<'a> {
    source: &'a str,
    section: Option<FixtureSection>,
    traits: Vec<TraitData>,
    structs: Vec<StructData>,
    impls: Vec<ImplData>,
    functions: Vec<FunctionData>,
    type_aliases: Vec<TypeAliasData>,
    trait_refs_by_name: HashMap<String, TraitDefRef>,
    type_refs_by_name: HashMap<String, TypeDefRef>,
}

impl<'a> TraitSelectionFixtureParser<'a> {
    fn resolved_one<T: PartialEq>(value: T) -> ExpectedUnique<T> {
        let mut resolved = ExpectedUnique::new();
        resolved.push(value);
        resolved
    }

    fn new(source: &'a str) -> Self {
        Self {
            source,
            section: None,
            traits: Vec::new(),
            structs: Vec::new(),
            impls: Vec::new(),
            functions: Vec::new(),
            type_aliases: Vec::new(),
            trait_refs_by_name: HashMap::new(),
            type_refs_by_name: HashMap::new(),
        }
    }

    fn parse(mut self) -> TraitSelectionFixture {
        for raw_line in self.source.lines() {
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }

            self.parse_line(line);
        }

        Self::fixture_with_traits_impls_aliases_and_structs(
            self.traits,
            self.impls,
            self.functions,
            self.type_aliases,
            self.structs,
        )
    }

    fn parse_line(&mut self, line: &str) {
        match line {
            "traits" => {
                self.section = Some(FixtureSection::Traits);
                return;
            }
            "structs" => {
                self.section = Some(FixtureSection::Structs);
                return;
            }
            "impls" => {
                self.section = Some(FixtureSection::Impls);
                return;
            }
            "functions" => {
                self.section = Some(FixtureSection::Functions);
                return;
            }
            "type aliases" => {
                self.section = Some(FixtureSection::TypeAliases);
                return;
            }
            _ => {}
        }

        match self
            .section
            .expect("fixture item should appear inside a section")
        {
            FixtureSection::Traits => self.parse_trait(line),
            FixtureSection::Structs => self.parse_struct(line),
            FixtureSection::Impls => self.parse_impl(line),
            FixtureSection::Functions => self.parse_function(line),
            FixtureSection::TypeAliases => self.parse_type_alias(line),
        }
    }

    fn parse_trait(&mut self, line: &str) {
        let (id, rest) = Self::parse_numbered_line(line, "trait#");
        assert_eq!(id, self.traits.len(), "trait fixture ids should be dense");
        let (rest, super_traits) = split_top_level_keyword(rest, ": ")
            .map(|(head, tail)| (head, parse_type_bounds(tail)))
            .unwrap_or((rest, Vec::new()));
        let (name, generics) = Self::parse_named_generics(rest);
        let trait_ref = trait_ref(id);
        self.trait_refs_by_name.insert(name.to_string(), trait_ref);
        let mut data = trait_data(id, name, generics);
        data.super_traits = super_traits;
        self.traits.push(data);
    }

    fn parse_struct(&mut self, line: &str) {
        let (id, rest) = Self::parse_numbered_line(line, "struct#");
        assert_eq!(id, self.structs.len(), "struct fixture ids should be dense");
        let (name, generics) = Self::parse_named_generics(rest);
        let def = type_def(id);
        self.type_refs_by_name.insert(name.to_string(), def);
        self.structs.push(struct_data(id, name, generics));
    }

    fn parse_impl(&mut self, line: &str) {
        let (id, rest) = Self::parse_numbered_line(line, "impl#");
        assert_eq!(id, self.impls.len(), "impl fixture ids should be dense");
        let (rest, note) = Self::split_trailing_note(rest);
        let rest = rest
            .strip_prefix("impl")
            .expect("impl fixture should start with `impl`")
            .trim();

        let (mut generics, rest) = Self::parse_leading_generics(rest);
        let (rest, where_predicates) = split_top_level_keyword(rest, " where ")
            .map(|(head, tail)| (head, Self::parse_where_predicates(tail)))
            .unwrap_or((rest, Vec::new()));
        generics.where_predicates.extend(where_predicates);

        let (trait_ty, self_ty) =
            split_top_level_keyword(rest, " for ").expect("impl fixture should contain ` for `");
        let trait_ty = parse_type_ref(trait_ty);
        let self_ty = parse_type_ref(self_ty);
        let trait_name =
            Self::type_ref_path_name(&trait_ty).expect("impl trait fixture should be a trait path");
        let trait_ref = *self
            .trait_refs_by_name
            .get(&trait_name)
            .unwrap_or_else(|| panic!("fixture should declare trait `{trait_name}` before impls"));

        // Some tests need an impl whose written self type is not a resolvable concrete type,
        // for example a macro-generated header or `impl<T> Trait for T` that should still be
        // visible as a `User` impl for the matcher. The optional note keeps that fact explicit in
        // the fixture instead of hiding it in Rust construction code.
        let resolved_self_ty = self.resolve_impl_self_ty(&self_ty, note.as_deref());
        let impl_data = ImplData {
            local_impl: Self::local_impl(id),
            source: dummy_source(),
            owner: module(),
            generics,
            trait_ref: Some(trait_ty),
            self_ty,
            resolved_self_ty,
            resolved_trait_ref: Self::resolved_one(trait_ref),
            items: Vec::new(),
            is_unsafe: false,
        };
        self.impls.push(impl_data);
    }

    fn parse_function(&mut self, line: &str) {
        let (id, rest) = Self::parse_numbered_line(line, "fn#");
        assert_eq!(
            id,
            self.functions.len(),
            "function fixture ids should be dense"
        );
        let (name, ret_ty) = rest
            .split_once(" -> ")
            .expect("function fixture should be written as `name -> ReturnType`");
        let (owner, name) = if let Some((trait_name, function_name)) = name.split_once("::") {
            let trait_ref = *self
                .trait_refs_by_name
                .get(trait_name)
                .unwrap_or_else(|| panic!("fixture should declare trait `{trait_name}` first"));
            self.traits[trait_ref.id.0]
                .items
                .push(AssocItemId::Function(FunctionId(id)));
            (ItemOwner::Trait(trait_ref.id), function_name)
        } else {
            (ItemOwner::Module(module()), name)
        };
        let (name, generics) = Self::parse_named_generics(name);
        self.functions.push(FunctionData {
            local_def: None,
            source: dummy_source(),
            span: fixture_span(),
            name_span: None,
            owner,
            name: Name::new(name),
            visibility: VisibilityLevel::Public,
            docs: None,
            signature: FunctionSignature::from_item(&FunctionItem {
                generics,
                params: Vec::new(),
                ret_ty: Some(parse_type_ref(ret_ty)),
                qualifiers: FunctionQualifiers::default(),
                has_body: false,
                proc_macro: None,
            }),
        });
    }

    fn parse_type_alias(&mut self, line: &str) {
        let (id, rest) = Self::parse_numbered_line(line, "type#");
        assert_eq!(
            id,
            self.type_aliases.len(),
            "type alias fixture ids should be dense"
        );
        let (owner_and_name, aliased_ty) = rest
            .split_once(" = ")
            .map(|(lhs, rhs)| (lhs, Some(parse_type_ref(rhs))))
            .unwrap_or((rest, None));
        let (owner_and_name, bounds) = split_top_level_keyword(owner_and_name, ": ")
            .map(|(owner_and_name, bounds)| (owner_and_name, parse_type_bounds(bounds)))
            .unwrap_or((owner_and_name, Vec::new()));
        let (owner, name) = owner_and_name
            .split_once("::")
            .expect("type alias fixture should be written as owner::Name");
        let owner = if let Some(index) = owner.strip_prefix("trait#") {
            let trait_id = TraitId(parse_usize(index, "trait type alias owner"));
            self.traits[trait_id.0]
                .items
                .push(AssocItemId::TypeAlias(TypeAliasId(id)));
            ItemOwner::Trait(trait_id)
        } else if let Some(index) = owner.strip_prefix("impl#") {
            let impl_id = ImplId(parse_usize(index, "impl type alias owner"));
            self.impls[impl_id.0]
                .items
                .push(AssocItemId::TypeAlias(TypeAliasId(id)));
            ItemOwner::Impl(impl_id)
        } else {
            panic!("type alias owner should be `trait#N` or `impl#N`");
        };

        self.type_aliases
            .push(type_alias_data(name, owner, bounds, aliased_ty));
    }

    fn resolve_impl_self_ty(
        &self,
        self_ty: &TypeRef,
        note: Option<&str>,
    ) -> ExpectedUnique<TypeDefRef> {
        if let Some(note) = note {
            let note = note
                .strip_prefix("resolved self: ")
                .expect("impl fixture note should be `resolved self: ...`");
            if note == "empty" {
                return ExpectedUnique::Empty;
            }
            let def = *self
                .type_refs_by_name
                .get(note)
                .unwrap_or_else(|| panic!("unknown resolved self type `{note}`"));
            return Self::resolved_one(def);
        }

        let name = Self::type_ref_path_name(self_ty)
            .expect("impl self type should be a type path or use a `resolved self` note");
        let def = *self
            .type_refs_by_name
            .get(&name)
            .unwrap_or_else(|| panic!("unknown impl self type `{name}`"));
        Self::resolved_one(def)
    }

    fn local_impl(index: usize) -> LocalImplRef {
        LocalImplRef {
            origin: origin(),
            local_impl: LocalImplId(index),
        }
    }

    fn fixture_with_traits_impls_aliases_and_structs(
        mut traits: Vec<TraitData>,
        impls: Vec<ImplData>,
        functions: Vec<FunctionData>,
        type_aliases: Vec<TypeAliasData>,
        mut structs: Vec<StructData>,
    ) -> TraitSelectionFixture {
        let mut def_map_builder = DefMapBuilder::new(target());
        let root_module = def_map_builder.alloc_module(ModuleData {
            name: None,
            name_span: None,
            docs: None,
            user_facing_attrs: Default::default(),
            visibility: Visibility::Public,
            parent: None,
            children: Vec::new(),
            local_defs: Vec::new(),
            impls: Vec::new(),
            imports: Vec::new(),
            unresolved_imports: Vec::new(),
            scope: Default::default(),
            origin: ModuleOrigin::Root { file_id: FileId(0) },
        });
        debug_assert_eq!(root_module, module().module);

        let mut scope = ModuleScopeBuilder::default();
        let mut local_defs = Vec::new();

        // The fixture language describes semantic items directly, but trait selection resolves names
        // through `ItemPathQuery` just like production code. Build the smallest root module scope that
        // can resolve declared structs and traits instead of relying on a production same-name
        // fallback.
        for struct_data in &mut structs {
            let local_def = def_map_builder.alloc_local_def(LocalDefData {
                module: root_module,
                name: struct_data.name.clone(),
                kind: LocalDefKind::Struct,
                namespaces: NamespaceSet::TYPES,
                visibility: VisibilityLevel::Public,
                source: struct_data.source,
                file_id: FileId(0),
                name_span: None,
                span: fixture_span(),
                user_facing_attrs: Default::default(),
            });
            let local_def_ref = LocalDefRef {
                origin: origin(),
                local_def,
            };
            struct_data.local_def = local_def_ref;
            local_defs.push(local_def);
            scope.insert_binding(
                &struct_data.name,
                Namespace::Types,
                ScopeBinding::new(
                    DefId::Local(local_def_ref),
                    Visibility::Public,
                    ScopeBindingProvenance::Direct,
                ),
            );
        }

        for trait_data in &mut traits {
            let local_def = def_map_builder.alloc_local_def(LocalDefData {
                module: root_module,
                name: trait_data.name.clone(),
                kind: LocalDefKind::Trait,
                namespaces: NamespaceSet::TYPES,
                visibility: VisibilityLevel::Public,
                source: trait_data.source,
                file_id: FileId(0),
                name_span: None,
                span: fixture_span(),
                user_facing_attrs: Default::default(),
            });
            let local_def_ref = LocalDefRef {
                origin: origin(),
                local_def,
            };
            trait_data.local_def = local_def_ref;
            local_defs.push(local_def);
            scope.insert_binding(
                &trait_data.name,
                Namespace::Types,
                ScopeBinding::new(
                    DefId::Local(local_def_ref),
                    Visibility::Public,
                    ScopeBindingProvenance::Direct,
                ),
            );
        }

        let module_data = def_map_builder
            .module_mut(root_module)
            .expect("fixture root module should exist");
        let local_def_count = local_defs.len();
        module_data.local_defs = local_defs;
        module_data.scope = scope.freeze();

        let mut builder = ItemStoreBuilder::new(origin(), local_def_count);
        for struct_data in structs {
            let local_def = struct_data.local_def.local_def;
            let struct_id = builder.structs.alloc(struct_data);
            builder.set_local_item(local_def, ItemId::Struct(struct_id));
        }
        for type_alias_data in type_aliases {
            builder.type_aliases.alloc(type_alias_data);
        }
        for trait_data in traits {
            let local_def = trait_data.local_def.local_def;
            let trait_id = builder.traits.alloc(trait_data);
            builder.set_local_item(local_def, ItemId::Trait(trait_id));
        }
        for impl_data in impls {
            builder.impls.alloc(impl_data);
        }
        for function_data in functions {
            builder.functions.alloc(function_data);
        }
        let mut fixture = TraitSelectionFixture {
            def_map: def_map_builder.build(),
            store: builder.build(),
            target: target(),
            lookup_index: ItemLookupIndex::default(),
            dependencies: Vec::new(),
            type_names: HashMap::new(),
            trait_names: HashMap::new(),
            type_refs_by_name: HashMap::new(),
            trait_refs_by_name: HashMap::new(),
        };
        for (struct_id, data) in fixture.store.structs().iter_with_ids() {
            let def = TypeDefRef {
                origin: origin(),
                id: TypeDefId::Struct(struct_id),
            };
            fixture.type_names.insert(def, data.name.to_string());
            fixture.type_refs_by_name.insert(data.name.to_string(), def);
        }
        for (trait_id, data) in fixture.store.traits().iter_with_ids() {
            let trait_ref = TraitDefRef {
                origin: origin(),
                id: trait_id,
            };
            fixture.trait_names.insert(trait_ref, data.name.to_string());
            fixture
                .trait_refs_by_name
                .insert(data.name.to_string(), trait_ref);
        }
        fixture.lookup_index = ItemLookupIndex::build_from_store(&fixture.store, &HashMap::new());
        fixture
    }

    fn type_ref_path_name(ty: &TypeRef) -> Option<String> {
        match ty {
            TypeRef::Path(path) => path.segments.last().map(|segment| segment.name.to_string()),
            TypeRef::Unit
            | TypeRef::Never
            | TypeRef::Infer
            | TypeRef::Tuple(_)
            | TypeRef::Array { .. }
            | TypeRef::Slice(_)
            | TypeRef::Reference { .. }
            | TypeRef::RawPointer { .. }
            | TypeRef::FnPointer { .. }
            | TypeRef::ImplTrait(_)
            | TypeRef::DynTrait(_)
            | TypeRef::Unknown(_) => None,
        }
    }

    fn parse_numbered_line<'line>(line: &'line str, prefix: &str) -> (usize, &'line str) {
        let rest = line
            .strip_prefix(prefix)
            .unwrap_or_else(|| panic!("fixture line should start with `{prefix}`: {line}"));
        let (index, rest) = rest
            .split_once(' ')
            .unwrap_or_else(|| panic!("fixture line should have a number and body: {line}"));
        (parse_usize(index, prefix), rest.trim())
    }

    fn split_trailing_note(line: &str) -> (&str, Option<String>) {
        let Some((head, note)) = line.rsplit_once(" [") else {
            return (line, None);
        };
        let note = note
            .strip_suffix(']')
            .unwrap_or_else(|| panic!("fixture note should end with `]`: {line}"));
        (head.trim(), Some(note.to_string()))
    }

    fn parse_named_generics(text: &str) -> (&str, GenericParams) {
        let Some(angle_start) = text.find('<') else {
            return (text.trim(), GenericParams::default());
        };
        let angle_end = matching_angle(text, angle_start);
        let name = text[..angle_start].trim();
        let generics = Self::parse_generic_params(&text[angle_start + 1..angle_end]);
        (name, generics)
    }

    fn parse_leading_generics(text: &str) -> (GenericParams, &str) {
        let text = text.trim();
        if !text.starts_with('<') {
            return (GenericParams::default(), text);
        }

        let angle_end = matching_angle(text, 0);
        (
            Self::parse_generic_params(&text[1..angle_end]),
            text[angle_end + 1..].trim(),
        )
    }

    fn parse_where_predicates(text: &str) -> Vec<WherePredicate> {
        split_top_level_commas(text)
            .into_iter()
            .map(|predicate| {
                let (ty, bounds) = split_top_level_keyword(predicate, ": ")
                    .expect("where predicate should be written as `Type: Bound`");
                WherePredicate::Type {
                    ty: parse_type_ref(ty),
                    bounds: parse_type_bounds(bounds),
                }
            })
            .collect()
    }

    fn parse_generic_params(text: &str) -> GenericParams {
        if text.trim().is_empty() {
            return GenericParams::default();
        }

        generics(
            split_top_level_commas(text)
                .into_iter()
                .map(Self::parse_type_param_decl)
                .collect(),
        )
    }

    fn parse_type_param_decl(text: &str) -> TypeParamData {
        if let Some((name, bounds)) = split_top_level_keyword(text, ": ") {
            return type_param_with_bounds(name.trim(), parse_type_bounds(bounds));
        }

        type_param(text.trim())
    }
}

fn parse_usize(text: &str, context: &str) -> usize {
    text.parse::<usize>()
        .unwrap_or_else(|_| panic!("{context} should be a usize: {text}"))
}

fn parse_type_bounds(text: &str) -> Vec<TypeBound> {
    split_top_level(text, '+')
        .into_iter()
        .map(|bound| {
            let bound = bound.trim();
            let (modifier, bound) = match bound.strip_prefix('?') {
                Some(bound) => (TraitBoundModifier::Maybe, bound),
                None => (TraitBoundModifier::None, bound),
            };
            TypeBound::Trait {
                ty: parse_type_ref(bound),
                modifier,
            }
        })
        .collect()
}

fn parse_type_ref(text: &str) -> TypeRef {
    let text = text.trim();
    if let Some(unsupported) = text
        .strip_prefix("<unsupported:")
        .and_then(|text| text.strip_suffix('>'))
    {
        return TypeRef::unknown_from_text(unsupported);
    }
    if text == "()" {
        return TypeRef::Unit;
    }
    if text == "!" {
        return TypeRef::Never;
    }
    if let Some(bounds) = text.strip_prefix("impl ") {
        return TypeRef::ImplTrait(parse_type_bounds(bounds));
    }
    if let Some(inner) = text.strip_prefix("*const ") {
        return TypeRef::RawPointer {
            mutability: rg_ir_model::Mutability::Shared,
            inner: Box::new(parse_type_ref(inner)),
        };
    }
    if let Some(inner) = text.strip_prefix("*mut ") {
        return TypeRef::RawPointer {
            mutability: rg_ir_model::Mutability::Mutable,
            inner: Box::new(parse_type_ref(inner)),
        };
    }
    if let Some(signature) = text.strip_prefix("fn(")
        && let Some((params, ret)) = signature.rsplit_once(") -> ")
    {
        let params = if params.trim().is_empty() {
            Vec::new()
        } else {
            split_top_level(params, ',')
                .into_iter()
                .map(parse_type_ref)
                .collect()
        };
        return TypeRef::FnPointer {
            params,
            ret: Box::new(parse_type_ref(ret)),
        };
    }
    if let Some(ty) = parse_bracket_ty(text) {
        return match ty {
            ParsedBracketTy::Slice(inner) => TypeRef::Slice(Box::new(parse_type_ref(inner))),
            ParsedBracketTy::Array { inner, len } => TypeRef::Array {
                inner: Box::new(parse_type_ref(inner)),
                len: len.map(|text| rg_item_tree::ConstExpr::new(text, fixture_span())),
            },
        };
    }
    if text.starts_with('<') {
        let angle_end = matching_angle(text, 0);
        if let Some(assoc_name) = text[angle_end + 1..].strip_prefix("::") {
            let inner = &text[1..angle_end];
            let (self_ty, trait_ty) = split_top_level_keyword(inner, " as ")
                .expect("qualified associated type should contain ` as `");
            return qualified_assoc_ty(
                parse_type_ref(self_ty),
                parse_type_ref(trait_ty),
                assoc_name,
            );
        }
    }

    let (name, args) = parse_path_head_and_args(text);
    path_ty(name, args.into_iter().map(parse_item_generic_arg).collect())
}

fn parse_item_generic_arg(text: &str) -> ItemGenericArg {
    if let Some((name, ty)) = split_top_level_keyword(text, " = ") {
        return ItemGenericArg::AssocType {
            name: Name::new(name),
            name_span: fixture_span(),
            ty: Some(parse_type_ref(ty)),
        };
    }

    type_arg(parse_type_ref(text))
}

enum ParsedBracketTy<'a> {
    Slice(&'a str),
    Array { inner: &'a str, len: Option<String> },
}

fn parse_bracket_ty(text: &str) -> Option<ParsedBracketTy<'_>> {
    if !text.starts_with('[') || !text.ends_with(']') {
        return None;
    }

    let body = &text[1..text.len() - 1];
    let parts = split_top_level(body, ';');
    match parts.as_slice() {
        [inner] => Some(ParsedBracketTy::Slice(inner)),
        [inner, len] => Some(ParsedBracketTy::Array {
            inner,
            len: (!len.is_empty() && *len != "_").then(|| (*len).to_owned()),
        }),
        _ => panic!("array type should contain at most one top-level `;`: {text}"),
    }
}

fn parse_path_head_and_args(text: &str) -> (&str, Vec<&str>) {
    let Some(angle_start) = text.find('<') else {
        return (text.trim(), Vec::new());
    };
    let angle_end = matching_angle(text, angle_start);
    assert_eq!(
        text[angle_end + 1..].trim(),
        "",
        "path type should not have tokens after generic args: {text}"
    );
    (
        text[..angle_start].trim(),
        split_top_level_commas(&text[angle_start + 1..angle_end]),
    )
}

fn split_top_level_keyword<'a>(text: &'a str, keyword: &str) -> Option<(&'a str, &'a str)> {
    let mut depth = 0i32;
    let mut index = 0usize;
    while index < text.len() {
        let rest = &text[index..];
        if depth == 0 && rest.starts_with(keyword) {
            return Some((text[..index].trim(), text[index + keyword.len()..].trim()));
        }

        let ch = rest
            .chars()
            .next()
            .expect("loop index should stay on char boundary");
        match ch {
            '<' | '(' | '[' | '{' => depth += 1,
            '>' | ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        index += ch.len_utf8();
    }

    None
}

fn split_top_level_commas(text: &str) -> Vec<&str> {
    split_top_level(text, ',')
}

fn split_top_level(text: &str, delimiter: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '<' | '(' | '[' | '{' => depth += 1,
            '>' | ')' | ']' | '}' => depth -= 1,
            ch if ch == delimiter && depth == 0 => {
                parts.push(text[start..index].trim());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(text[start..].trim());
    parts
}

fn matching_angle(text: &str, start: usize) -> usize {
    let mut depth = 0i32;
    for (index, ch) in text[start..].char_indices() {
        let index = start + index;
        match ch {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return index;
                }
            }
            _ => {}
        }
    }

    panic!("unclosed angle bracket in `{text}`");
}

pub(super) struct TraitSelectionCase {
    title: &'static str,
    kind: TraitSelectionCaseKind,
}

impl TraitSelectionCase {
    pub(super) fn probe(title: &'static str, goal: impl Into<String>) -> Self {
        Self {
            title,
            kind: TraitSelectionCaseKind::Probe(goal.into()),
        }
    }

    pub(super) fn normalize_assoc(title: &'static str, goal: impl Into<String>) -> Self {
        Self {
            title,
            kind: TraitSelectionCaseKind::NormalizeAssoc(goal.into()),
        }
    }

    fn query_name(&self) -> &'static str {
        "selection"
    }
}

enum TraitSelectionCaseKind {
    Probe(String),
    NormalizeAssoc(String),
}

pub(super) fn check_trait_selection_queries(
    fixture: impl Into<TraitSelectionFixture>,
    cases: Vec<TraitSelectionCase>,
    expect: Expect,
) {
    let snapshot = TraitSelectionSnapshot {
        fixture: fixture.into(),
        cases,
    };
    let actual = format!("{}\n", snapshot.render().trim_end());
    expect.assert_eq(&actual);
}

/// Render shared solver operations while their tables are still alive, so cases can inspect
/// named variables such as `?Item` after selection or normalization. The owned query adapter
/// finalizes those variables before returning and would hide the relationships being tested.
struct TraitSelectionSnapshot {
    fixture: TraitSelectionFixture,
    cases: Vec<TraitSelectionCase>,
}

impl TraitSelectionSnapshot {
    fn render(&self) -> String {
        let mut dump = String::new();
        for (idx, case) in self.cases.iter().enumerate() {
            if idx > 0 {
                writeln!(dump).expect("string writes should not fail");
            }
            self.render_case(case, &mut dump);
        }

        dump
    }

    fn render_case(&self, case: &TraitSelectionCase, dump: &mut String) {
        writeln!(dump, "{}", case.title).expect("string writes should not fail");
        writeln!(dump, "  query: {}", case.query_name()).expect("string writes should not fail");

        match &case.kind {
            TraitSelectionCaseKind::Probe(goal) => self.render_probe_case(goal, dump),
            TraitSelectionCaseKind::NormalizeAssoc(goal) => self.render_normalize_case(goal, dump),
        }
    }

    fn render_probe_case(&self, goal: &str, dump: &mut String) {
        let context = TyContext::new(
            &self.fixture,
            &self.fixture,
            self.fixture.lookup_query(),
            self.fixture.target,
            rg_std::CancellationToken::new(),
        );
        solver::SemanticDeclarations::new(&context, context.item_paths())
            .with_solver(|solver| {
                let table = InferenceTable::new(solver, Default::default());
                let cx = table.interner();
                let parsed = TraitSelectionQueryParser::new(&self.fixture, &table).parse_goal(goal);
                writeln!(
                    dump,
                    "  goal: {}",
                    self.render_live_goal(cx, &parsed.goal, &parsed.vars)
                )
                .expect("string write");
                let application = parsed.goal.application;
                let bindings = parsed
                    .goal
                    .associated_types
                    .iter()
                    .map(|bound| application.associated_type_eq(cx, bound.associated_ty, bound.ty))
                    .collect::<Vec<_>>();
                let candidates = crate::lookup::trait_impl_candidates(
                    &context,
                    application.def,
                    &cx.raise_ty(application.self_ty().expect("Self"))
                        .unwrap_or(Ty::Unknown),
                )
                .expect("fixture lookup");
                let selected = table.select_trait_impl(
                    application,
                    &bindings,
                    candidates.into_iter().map(|candidate| candidate.impl_ref),
                );
                match selected {
                    ExpectedUnique::Empty => {
                        writeln!(dump, "  result: empty").expect("string write")
                    }
                    ExpectedUnique::Ambiguous => {
                        writeln!(dump, "  result: ambiguous").expect("string write")
                    }
                    ExpectedUnique::One(selected) => {
                        writeln!(
                            dump,
                            "  result: one\n    impl: impl#{}\n    applicability: {}",
                            selected.impl_ref.id.0,
                            if selected.outcome == Outcome::Proven {
                                "yes"
                            } else {
                                "maybe"
                            }
                        )
                        .expect("string write");
                        self.render_named_vars(&parsed.vars, &selected.table, dump);
                    }
                }
            })
            .expect("fixture declarations load");
    }

    fn render_normalize_case(&self, goal: &str, dump: &mut String) {
        let context = TyContext::new(
            &self.fixture,
            &self.fixture,
            self.fixture.lookup_query(),
            self.fixture.target,
            rg_std::CancellationToken::new(),
        );
        solver::SemanticDeclarations::new(&context, context.item_paths())
            .with_solver(|solver| {
                let table = InferenceTable::new(solver, Default::default());
                let cx = table.interner();
                let parsed =
                    TraitSelectionQueryParser::new(&self.fixture, &table).parse_assoc_goal(goal);
                writeln!(
                    dump,
                    "  goal: <{} as {}>::{}",
                    self.render_live_ty(
                        cx,
                        parsed.goal.application.self_ty().expect("Self"),
                        &parsed.vars
                    ),
                    self.render_live_trait(cx, &parsed.goal, &parsed.vars),
                    parsed.assoc_name
                )
                .expect("string write");
                let application = parsed.goal.application;
                let bindings = parsed
                    .goal
                    .associated_types
                    .iter()
                    .map(|bound| application.associated_type_eq(cx, bound.associated_ty, bound.ty))
                    .collect::<Vec<_>>();
                let associated_ty = context
                    .item_paths()
                    .items()
                    .declared_associated_type_by_name(application.def, &parsed.assoc_name)
                    .expect("fixture lookup")
                    .expect("fixture alias");
                let Some((ty, outcome)) =
                    table.normalize_assoc_type(application, &bindings, associated_ty)
                else {
                    writeln!(dump, "  result: none").expect("string write");
                    return;
                };
                writeln!(
                    dump,
                    "  result: projected\n    final: {}\n    applicability: {}",
                    self.render_ty(&table.finalize(ty)),
                    if outcome == Outcome::Proven {
                        "yes"
                    } else {
                        "maybe"
                    }
                )
                .expect("string write");
                self.render_named_vars(&parsed.vars, &table, dump);
            })
            .expect("fixture declarations load");
    }

    fn render_associated_ty_name(&self, associated_ty: TypeAliasRef) -> String {
        self.fixture
            .store
            .type_alias_data(associated_ty.id)
            .map(|data| data.name.to_string())
            .unwrap_or_else(|| format!("type#{}", associated_ty.id.0))
    }

    fn render_opaque(&self, opaque: &OpaqueTy) -> String {
        let mut bounds = SemanticSignatureQuery::new(&self.fixture, &self.fixture)
            .opaque_bounds(opaque)
            .expect("fixture opaque bounds")
            .unwrap_or_default()
            .into_iter()
            .map(|bound| {
                let mut args = bound
                    .application
                    .args
                    .iter()
                    .skip(1)
                    .map(|arg| self.render_generic_arg(arg))
                    .collect::<Vec<_>>();
                args.extend(bound.associated_types.iter().map(|binding| {
                    format!(
                        "{} = {}",
                        self.render_associated_ty_name(binding.associated_ty),
                        self.render_ty(&binding.ty)
                    )
                }));
                let name = self.render_trait_ref(bound.application.def);
                if args.is_empty() {
                    name
                } else {
                    format!("{name}<{}>", args.join(", "))
                }
            })
            .collect::<Vec<_>>();
        bounds.sort();
        if bounds.is_empty() {
            "impl _".to_string()
        } else {
            format!("impl {}", bounds.join(" + "))
        }
    }

    fn render_trait_ref(&self, trait_ref: TraitDefRef) -> String {
        if trait_ref.origin != origin() {
            return format!("{trait_ref:?}");
        }

        if let Some(name) = self.fixture.trait_names.get(&trait_ref) {
            return name.clone();
        }

        self.fixture
            .store
            .trait_data(trait_ref.id)
            .map(|data| data.name.to_string())
            .unwrap_or_else(|| format!("trait#{}", trait_ref.id.0))
    }

    fn render_type_def_ref(&self, def: TypeDefRef) -> String {
        if def.origin != origin() {
            return format!("{def:?}");
        }

        if let Some(name) = self.fixture.type_names.get(&def) {
            return name.clone();
        }

        match def.id {
            TypeDefId::Struct(id) => self
                .fixture
                .store
                .struct_data(id)
                .map(|data| data.name.to_string())
                .unwrap_or_else(|| format!("struct#{}", id.0)),
            TypeDefId::Union(id) => format!("union#{}", id.0),
            TypeDefId::Enum(id) => format!("enum#{}", id.0),
        }
    }

    fn render_named_vars<'s>(
        &self,
        vars: &[NamedInferVar<'s>],
        table: &InferenceTable<'s>,
        dump: &mut String,
    ) {
        if vars.is_empty() {
            return;
        }
        writeln!(dump, "    vars").expect("string write");
        for var in vars {
            let result = self.render_ty(&table.finalize(var.ty));
            writeln!(dump, "      ?{} = {result}", var.name).expect("string write");
        }
    }

    fn render_live_goal<'s>(
        &self,
        cx: solver::SolverInterner<'s>,
        goal: &solver::TraitRefLowering<'s>,
        vars: &[NamedInferVar<'s>],
    ) -> String {
        format!(
            "{}: {}",
            self.render_live_ty(cx, goal.application.self_ty().expect("Self"), vars),
            self.render_live_trait(cx, goal, vars)
        )
    }

    fn render_live_trait<'s>(
        &self,
        cx: solver::SolverInterner<'s>,
        goal: &solver::TraitRefLowering<'s>,
        vars: &[NamedInferVar<'s>],
    ) -> String {
        let name = self.render_trait_ref(goal.application.def);
        let mut args = goal
            .application
            .args
            .iter()
            .skip(1)
            .map(|arg| self.render_live_arg(cx, arg, vars))
            .collect::<Vec<_>>();
        args.extend(goal.associated_types.iter().map(|binding| {
            format!(
                "{} = {}",
                self.render_associated_ty_name(binding.associated_ty),
                self.render_live_ty(cx, binding.ty, vars)
            )
        }));
        if args.is_empty() {
            name
        } else {
            format!("{name}<{}>", args.join(", "))
        }
    }

    fn render_live_args<'s>(
        &self,
        cx: solver::SolverInterner<'s>,
        args: solver::GenericArgs<'s>,
        vars: &[NamedInferVar<'s>],
    ) -> String {
        if args.is_empty() {
            return String::new();
        }
        format!(
            "<{}>",
            args.iter()
                .map(|arg| self.render_live_arg(cx, arg, vars))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    fn render_live_arg<'s>(
        &self,
        cx: solver::SolverInterner<'s>,
        arg: solver::GenericArg<'s>,
        vars: &[NamedInferVar<'s>],
    ) -> String {
        match arg.as_ty() {
            Some(ty) => self.render_live_ty(cx, ty, vars),
            None => self.render_generic_arg(&cx.raise_args(solver::List::new(cx, &[arg]))[0]),
        }
    }

    fn render_live_ty<'s>(
        &self,
        cx: solver::SolverInterner<'s>,
        ty: solver::Ty<'s>,
        vars: &[NamedInferVar<'s>],
    ) -> String {
        if let Some(var) = vars.iter().find(|var| var.ty == ty) {
            return format!("?{}", var.name);
        }
        use solver::TyShape as S;
        match ty.shape() {
            S::Tuple(fields) => {
                let mut fields = fields
                    .iter()
                    .map(|field| self.render_live_ty(cx, field, vars))
                    .collect::<Vec<_>>();
                if fields.len() == 1 {
                    fields[0].push(',');
                }
                format!("({})", fields.join(", "))
            }
            S::Array { inner, len } => format!(
                "[{}; {}]",
                self.render_live_ty(cx, inner, vars),
                cx.raise_const(len)
            ),
            S::Slice(inner) => format!("[{}]", self.render_live_ty(cx, inner, vars)),
            S::Reference {
                mutability, inner, ..
            } => format!(
                "{}{}",
                mutability.render_prefix(),
                self.render_live_ty(cx, inner, vars)
            ),
            S::RawPointer { mutability, inner } => {
                let qualifier = if mutability == rg_ir_model::Mutability::Mutable {
                    "mut"
                } else {
                    "const"
                };
                format!("*{qualifier} {}", self.render_live_ty(cx, inner, vars))
            }
            S::FnPointer { params, ret } => format!(
                "fn({}) -> {}",
                params
                    .iter()
                    .map(|ty| self.render_live_ty(cx, ty, vars))
                    .collect::<Vec<_>>()
                    .join(", "),
                self.render_live_ty(cx, ret, vars)
            ),
            S::FnDef(function) => format!(
                "{{fn-item:{:?}{}}}",
                function.def,
                self.render_live_args(cx, function.args, vars)
            ),
            S::Adt(adt) => format!(
                "{}{}",
                self.render_type_def_ref(adt.def),
                self.render_live_args(cx, adt.args, vars)
            ),
            S::Alias(solver::AliasTy::Projection(alias)) => format!(
                "projection {}{}",
                self.render_associated_ty_name(alias.associated_ty),
                self.render_live_args(cx, alias.args, vars)
            ),
            _ => self.render_ty(&cx.raise_ty(ty).unwrap_or(Ty::Unknown)),
        }
    }

    fn render_ty(&self, ty: &Ty) -> String {
        match ty {
            Ty::Unit => "()".to_string(),
            Ty::Never => "!".to_string(),
            Ty::Primitive(primitive) => Self::render_primitive(*primitive),
            Ty::Tuple(fields) => self.render_tuple(fields, Self::render_ty),
            Ty::Array { inner, len } => {
                format!("[{}; {}]", self.render_ty(inner), len)
            }
            Ty::Slice(inner) => format!("[{}]", self.render_ty(inner)),
            Ty::Reference {
                mutability, inner, ..
            } => {
                format!("{}{}", mutability.render_prefix(), self.render_ty(inner))
            }
            Ty::RawPointer { mutability, inner } => {
                let qualifier = if matches!(mutability, rg_ir_model::Mutability::Mutable) {
                    "mut"
                } else {
                    "const"
                };
                format!("*{qualifier} {}", self.render_ty(inner))
            }
            Ty::FnPointer { params, ret } => {
                let params = params
                    .iter()
                    .map(|param| self.render_ty(param))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("fn({params}) -> {}", self.render_ty(ret))
            }
            Ty::Closure(closure) => format!("{{closure#{}}}", closure.id),
            Ty::FnDef(function) => format!(
                "{{fn-item:{:?}{}}}",
                function.def,
                self.render_generic_args(&function.args)
            ),
            Ty::Adt(ty) => self.render_nominal_ty(ty),
            Ty::Param(param) => self.render_type_param(*param),
            Ty::Alias(AliasTy::Projection(alias)) => format!(
                "projection {}{}",
                self.render_associated_ty_name(alias.associated_ty),
                self.render_generic_args(&alias.args)
            ),
            Ty::Alias(AliasTy::Opaque(opaque)) => self.render_opaque(opaque),
            Ty::Unknown => "_".to_string(),
        }
    }

    fn render_tuple<T>(&self, fields: &[T], render_field: fn(&Self, &T) -> String) -> String {
        if fields.is_empty() {
            return "()".to_string();
        }

        let mut rendered = fields
            .iter()
            .map(|field| render_field(self, field))
            .collect::<Vec<_>>();
        if rendered.len() == 1 {
            rendered[0].push(',');
        }

        format!("({})", rendered.join(", "))
    }

    fn render_nominal_ty(&self, ty: &AdtTy) -> String {
        let name = self.render_type_def_ref(ty.def);
        if ty.args.is_empty() {
            return name;
        }

        format!(
            "{}<{}>",
            name,
            ty.args
                .iter()
                .map(|arg| self.render_generic_arg(arg))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    fn render_type_param(&self, param: rg_ir_model::TypeParamRef) -> String {
        let generics = GenericsQuery::new(&self.fixture)
            .generics(param.owner)
            .expect("fixture generic declarations should be available while rendering a type");
        generics
            .iter()
            .find(|data| data.param() == GenericParamRef::Type(param))
            .map(|data| match data.source() {
                GenericParamSource::Type(source) => source.name.to_string(),
                GenericParamSource::TraitSelf => "Self".to_string(),
                GenericParamSource::ArgumentImplTrait(_) => "<argument impl Trait>".to_string(),
                GenericParamSource::Lifetime(_) | GenericParamSource::Const(_) => {
                    unreachable!("a type parameter should have type-like provenance")
                }
            })
            .unwrap_or_else(|| "<missing-param>".to_string())
    }

    fn render_generic_args(&self, args: &[GenericArg]) -> String {
        if args.is_empty() {
            return String::new();
        }
        format!(
            "<{}>",
            args.iter()
                .map(|arg| self.render_generic_arg(arg))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    fn render_generic_arg(&self, arg: &GenericArg) -> String {
        match arg {
            GenericArg::Type(ty) => self.render_ty(ty),
            GenericArg::Lifetime(lifetime) => lifetime.to_string(),
            GenericArg::Const(value) => value.to_string(),
        }
    }

    fn render_primitive(primitive: PrimitiveTy) -> String {
        match primitive {
            PrimitiveTy::Bool => "bool".to_string(),
            PrimitiveTy::Char => "char".to_string(),
            PrimitiveTy::Str => "str".to_string(),
            PrimitiveTy::SignedInt(kind) => match kind {
                SignedIntTy::I8 => "i8",
                SignedIntTy::I16 => "i16",
                SignedIntTy::I32 => "i32",
                SignedIntTy::I64 => "i64",
                SignedIntTy::I128 => "i128",
                SignedIntTy::Isize => "isize",
            }
            .to_string(),
            PrimitiveTy::UnsignedInt(kind) => match kind {
                UnsignedIntTy::U8 => "u8",
                UnsignedIntTy::U16 => "u16",
                UnsignedIntTy::U32 => "u32",
                UnsignedIntTy::U64 => "u64",
                UnsignedIntTy::U128 => "u128",
                UnsignedIntTy::Usize => "usize",
            }
            .to_string(),
            PrimitiveTy::Float(kind) => match kind {
                FloatTy::F32 => "f32",
                FloatTy::F64 => "f64",
            }
            .to_string(),
        }
    }
}

pub(super) struct ParsedTraitQuery<'s> {
    pub(super) goal: solver::TraitRefLowering<'s>,
    vars: Vec<NamedInferVar<'s>>,
}

pub(super) struct ParsedAssocQuery<'s> {
    pub(super) goal: solver::TraitRefLowering<'s>,
    pub(super) assoc_name: String,
    vars: Vec<NamedInferVar<'s>>,
}

struct NamedInferVar<'s> {
    name: String,
    ty: solver::Ty<'s>,
}

pub(super) struct TraitSelectionQueryParser<'a, 's> {
    fixture: &'a TraitSelectionFixture,
    vars: Vec<NamedInferVar<'s>>,
    table: &'a InferenceTable<'s>,
}

impl<'a, 's> TraitSelectionQueryParser<'a, 's> {
    pub(super) fn new(fixture: &'a TraitSelectionFixture, table: &'a InferenceTable<'s>) -> Self {
        Self {
            fixture,
            vars: Vec::new(),
            table,
        }
    }

    pub(super) fn parse_goal(mut self, text: &str) -> ParsedTraitQuery<'s> {
        let (self_ty, trait_path) = split_top_level_keyword(text, ": ")
            .expect("trait query should be written as `Self: Trait<Args>`");
        let (trait_ref, mut args, associated_types) = self.parse_trait_path(trait_path);
        args.insert(0, self.parse_infer_ty(self_ty).into());
        let goal = solver::TraitRefLowering {
            application: solver::TraitApplication {
                def: trait_ref,
                args: solver::List::new(self.table.interner(), &args),
            },
            associated_types,
        };
        ParsedTraitQuery {
            goal,
            vars: self.vars,
        }
    }

    pub(super) fn parse_assoc_goal(mut self, text: &str) -> ParsedAssocQuery<'s> {
        let text = text.trim();
        assert!(
            text.starts_with('<'),
            "associated projection query should start with `<`: {text}"
        );
        let angle_end = matching_angle(text, 0);
        let assoc_name = text[angle_end + 1..].strip_prefix("::").unwrap_or_else(|| {
            panic!("associated projection query should end with `::Assoc`: {text}")
        });
        let inner = &text[1..angle_end];
        let (self_ty, trait_path) = split_top_level_keyword(inner, " as ")
            .expect("associated projection query should contain ` as `");
        let (trait_ref, mut args, associated_types) = self.parse_trait_path(trait_path);
        args.insert(0, self.parse_infer_ty(self_ty).into());
        let goal = solver::TraitRefLowering {
            application: solver::TraitApplication {
                def: trait_ref,
                args: solver::List::new(self.table.interner(), &args),
            },
            associated_types,
        };
        ParsedAssocQuery {
            goal,
            assoc_name: assoc_name.to_string(),
            vars: self.vars,
        }
    }

    fn parse_trait_path(
        &mut self,
        text: &str,
    ) -> (
        TraitDefRef,
        Vec<solver::GenericArg<'s>>,
        Vec<solver::AssocTypeBinding<'s>>,
    ) {
        let (trait_name, args) = parse_path_head_and_args(text.trim());
        let trait_ref = self
            .fixture
            .trait_ref_by_name(trait_name)
            .unwrap_or_else(|| panic!("query refers to unknown trait `{trait_name}`"));
        let mut positional = Vec::new();
        let mut associated_types = Vec::new();
        for arg in args {
            if let Some((name, ty)) = split_top_level_keyword(arg, " = ") {
                let associated_ty = self
                    .fixture
                    .associated_ty_by_name(trait_ref, name)
                    .unwrap_or_else(|| {
                        panic!(
                            "query refers to unknown associated type `{name}` on trait `{trait_name}`"
                        )
                    });
                associated_types.push(solver::AssocTypeBinding {
                    associated_ty,
                    ty: self.parse_infer_ty(ty),
                });
            } else {
                positional.push(self.parse_infer_ty(arg).into());
            }
        }
        (trait_ref, positional, associated_types)
    }

    fn parse_infer_ty(&mut self, text: &str) -> solver::Ty<'s> {
        let cx = self.table.interner();
        let text = text.trim();
        if text == "_" {
            return cx.unknown();
        }
        if let Some(name) = text.strip_prefix('?') {
            return self.type_var(name);
        }
        if let Some(index) = text.strip_prefix("opaque#") {
            let function = FunctionRef {
                origin: origin(),
                id: FunctionId(parse_usize(index, "opaque function id")),
            };
            let paths = ItemPathQuery::new(self.fixture, self.fixture);
            return TypeLoweringQuery::new(&paths, &paths)
                .function(cx, function)
                .expect("fixture opaque signature should lower")
                .unwrap_or_else(|| panic!("query refers to unknown opaque function `{index}`"))
                .ret;
        }
        if let Some(ty) = parse_bracket_ty(text) {
            return match ty {
                ParsedBracketTy::Slice(inner) => cx.slice(self.parse_infer_ty(inner)),
                ParsedBracketTy::Array { inner, len } => {
                    cx.array(self.parse_infer_ty(inner), cx.lower_const(len.into(), &[]))
                }
            };
        }

        let (name, args) = parse_path_head_and_args(text);
        let def = self
            .fixture
            .type_ref_by_name(name)
            .unwrap_or_else(|| panic!("query refers to unknown type `{name}`"));
        let args = args
            .into_iter()
            .map(|arg| self.parse_infer_ty(arg).into())
            .collect::<Vec<_>>();
        cx.adt(solver::AdtTy {
            def,
            args: solver::List::new(cx, &args),
        })
    }

    fn type_var(&mut self, name: &str) -> solver::Ty<'s> {
        if let Some(var) = self.vars.iter().find(|var| var.name == name) {
            return var.ty;
        }
        let ty = self.table.new_type_var();
        self.vars.push(NamedInferVar {
            name: name.to_string(),
            ty,
        });
        ty
    }
}

pub(super) fn prove_fixture_goal(fixture: &TraitSelectionFixture, goal: &str) -> Outcome {
    let context = TyContext::new(
        fixture,
        fixture,
        fixture.lookup_query(),
        fixture.target,
        rg_std::CancellationToken::new(),
    );
    solver::SemanticDeclarations::new(&context, context.item_paths())
        .with_solver(|solver| {
            let table = InferenceTable::new(solver, Default::default());
            let parsed = TraitSelectionQueryParser::new(fixture, &table).parse_goal(goal);
            table.prove([parsed.goal.application.clause(table.interner())])
        })
        .expect("fixture declarations load")
}
