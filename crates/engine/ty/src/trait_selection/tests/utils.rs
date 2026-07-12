use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt::{Debug, Write as _};

use expect_test::Expect;
use rg_ir_model::hir::items::{ImplData, StructData, TraitData, TypeAliasData};
use rg_ir_model::hir::signature::TypeAliasSignature;
use rg_ir_model::hir::source::{GeneratedItemRef, GeneratedSourceId, ItemSource, ItemSourceKind};
use rg_ir_model::items::{
    FieldList, FloatTy, GenericArg as ItemGenericArg, GenericParams, ItemTreeId, SignedIntTy,
    TypeAliasItem, TypeBound, TypeParamData, TypePath, TypePathAnchor, TypePathSegment, TypeRef,
    UnsignedIntTy, VisibilityLevel, WherePredicate,
};
use rg_ir_model::{
    AssocItemId, DefId, DefMapRef, FileId, ImplId, ItemId, ItemOwner, LocalDefId, LocalDefRef,
    LocalImplId, LocalImplRef, ModuleId, ModuleRef, PackageSlot, Span, StructId, TargetId,
    TargetRef, TextSpan, TraitApplicability, TraitId, TraitRef, TypeAliasId, TypeDefId, TypeDefRef,
};
use rg_ir_storage::{
    DefMap, DefMapBuilder, DefMapSource, ItemLookupIndex, ItemStore, ItemStoreBuilder,
    ItemStoreSource, LocalDefData, LocalDefKind, ModuleData, ModuleOrigin, ModuleScopeBuilder,
    Namespace, ScopeBinding, ScopeBindingOrigin, TargetItemQuery, TypePathContext,
};
use rg_std::ExpectedUnique;
use rg_text::Name;

use super::super::{ChalkTraitSolver, TraitGoal, TraitSelectionOptions, TraitSelectionQuery};
use crate::inference::{InferVarKind, InferenceTable};
use crate::{GenericArg, ItemPathQuery, NominalTy, OpaqueTraitBound, PrimitiveTy, Ty};

pub(super) struct TraitSelectionFixture {
    def_map: DefMap,
    pub(super) store: ItemStore,
    pub(super) target: TargetRef,
    lookup_index: ItemLookupIndex,
    type_names: HashMap<TypeDefRef, String>,
    trait_names: HashMap<TraitRef, String>,
    type_refs_by_name: HashMap<String, TypeDefRef>,
    trait_refs_by_name: HashMap<String, TraitRef>,
}

impl TraitSelectionFixture {
    // Tests use a small declarative fixture language instead of full Rust source. That keeps the
    // unit tests close to the trait-selection data model while still making the setup readable in
    // snapshots and reviews.
    pub(super) fn new(source: &str) -> Self {
        TraitSelectionFixtureParser::new(source).parse()
    }

    fn type_ref_by_name(&self, name: &str) -> Option<TypeDefRef> {
        self.type_refs_by_name.get(name).copied()
    }

    fn trait_ref_by_name(&self, name: &str) -> Option<TraitRef> {
        self.trait_refs_by_name.get(name).copied()
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
        Ok((origin_ref == origin()).then_some(&self.def_map))
    }

    fn extern_root(
        &self,
        _target: TargetRef,
        _name: &str,
    ) -> Result<Option<ModuleRef>, Self::Error> {
        Ok(None)
    }

    fn extern_roots(&self, _target: TargetRef) -> Result<Vec<(String, ModuleRef)>, Self::Error> {
        Ok(Vec::new())
    }

    fn prelude_module(&self, _target: TargetRef) -> Result<Option<ModuleRef>, Self::Error> {
        Ok(None)
    }

    fn root_module(&self, _target: TargetRef) -> Result<Option<ModuleRef>, Self::Error> {
        Ok(None)
    }
}

impl<'a> ItemStoreSource<'a> for &'a TraitSelectionFixture {
    type Error = Infallible;

    fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&'a ItemStore>, Self::Error> {
        Ok((origin == DefMapRef::Target(self.target)).then_some(&self.store))
    }

    fn included_stores(&self) -> Result<Vec<&'a ItemStore>, Self::Error> {
        Ok(vec![&self.store])
    }
}

pub(super) fn target() -> TargetRef {
    TargetRef {
        package: PackageSlot(0),
        target: TargetId(0),
    }
}

pub(super) fn origin() -> DefMapRef {
    DefMapRef::Target(target())
}

pub(super) fn module() -> ModuleRef {
    ModuleRef {
        origin: origin(),
        module: ModuleId(0),
    }
}

fn local_impl(index: usize) -> LocalImplRef {
    LocalImplRef {
        origin: origin(),
        local_impl: LocalImplId(index),
    }
}

pub(super) fn type_def(index: usize) -> TypeDefRef {
    TypeDefRef {
        origin: origin(),
        id: TypeDefId::Struct(StructId(index)),
    }
}

pub(super) fn trait_ref(index: usize) -> TraitRef {
    TraitRef {
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
    Span {
        text: TextSpan { start: 0, end: 0 },
    }
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
        types,
        ..GenericParams::default()
    }
}

pub(super) fn nominal_infer_ty(def: TypeDefRef, args: Vec<GenericArg>) -> Ty {
    Ty::Nominal(NominalTy { def, args })
}

fn resolved_one<T: PartialEq>(value: T) -> ExpectedUnique<T> {
    let mut resolved = ExpectedUnique::new();
    resolved.push(value);
    resolved
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
    aliased_ty: Option<TypeRef>,
) -> TypeAliasData {
    TypeAliasData {
        local_def: None,
        source: dummy_source(),
        span: Span {
            text: TextSpan { start: 0, end: 0 },
        },
        name_span: None,
        owner,
        name: Name::new(name),
        visibility: VisibilityLevel::Public,
        docs: None,
        signature: TypeAliasSignature::from_item(&TypeAliasItem {
            generics: GenericParams::default(),
            bounds: Vec::new(),
            aliased_ty,
        }),
    }
}

fn fixture_with_traits_impls_aliases_and_structs(
    mut traits: Vec<TraitData>,
    impls: Vec<ImplData>,
    type_aliases: Vec<TypeAliasData>,
    mut structs: Vec<StructData>,
) -> TraitSelectionFixture {
    let mut def_map_builder = DefMapBuilder::new(target());
    let root_module = def_map_builder.alloc_module(ModuleData {
        name: None,
        name_span: None,
        docs: None,
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
            visibility: VisibilityLevel::Public,
            source: struct_data.source,
            file_id: FileId(0),
            name_span: None,
            span: fixture_span(),
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
            ScopeBinding {
                def: DefId::Local(local_def_ref),
                visibility: VisibilityLevel::Public,
                owner: module(),
                origin: ScopeBindingOrigin::Direct,
            },
        );
    }

    for trait_data in &mut traits {
        let local_def = def_map_builder.alloc_local_def(LocalDefData {
            module: root_module,
            name: trait_data.name.clone(),
            kind: LocalDefKind::Trait,
            visibility: VisibilityLevel::Public,
            source: trait_data.source,
            file_id: FileId(0),
            name_span: None,
            span: fixture_span(),
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
            ScopeBinding {
                def: DefId::Local(local_def_ref),
                visibility: VisibilityLevel::Public,
                owner: module(),
                origin: ScopeBindingOrigin::Direct,
            },
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
    let mut fixture = TraitSelectionFixture {
        def_map: def_map_builder.build(),
        store: builder.build(),
        target: target(),
        lookup_index: ItemLookupIndex::default(),
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
        let trait_ref = TraitRef {
            origin: origin(),
            id: trait_id,
        };
        fixture.trait_names.insert(trait_ref, data.name.to_string());
        fixture
            .trait_refs_by_name
            .insert(data.name.to_string(), trait_ref);
    }
    {
        let target_items = TargetItemQuery::new(&fixture, &fixture, fixture.target);
        fixture.lookup_index =
            ItemLookupIndex::build_from(&target_items).expect("fixture lookup index should build");
    }
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

pub(super) fn query(
    fixture: &TraitSelectionFixture,
) -> TraitSelectionQuery<'_, &TraitSelectionFixture, &TraitSelectionFixture> {
    TraitSelectionQuery::with_index(
        ItemPathQuery::new(fixture, fixture),
        TargetItemQuery::new(fixture, fixture, fixture.target),
        &fixture.lookup_index,
    )
}

#[derive(Clone, Copy)]
enum FixtureSection {
    Traits,
    Structs,
    Impls,
    TypeAliases,
}

struct TraitSelectionFixtureParser<'a> {
    source: &'a str,
    section: Option<FixtureSection>,
    traits: Vec<TraitData>,
    structs: Vec<StructData>,
    impls: Vec<ImplData>,
    type_aliases: Vec<TypeAliasData>,
    trait_refs_by_name: HashMap<String, TraitRef>,
    type_refs_by_name: HashMap<String, TypeDefRef>,
}

impl<'a> TraitSelectionFixtureParser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            section: None,
            traits: Vec::new(),
            structs: Vec::new(),
            impls: Vec::new(),
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

        fixture_with_traits_impls_aliases_and_structs(
            self.traits,
            self.impls,
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
            FixtureSection::TypeAliases => self.parse_type_alias(line),
        }
    }

    fn parse_trait(&mut self, line: &str) {
        let (id, rest) = parse_numbered_line(line, "trait#");
        assert_eq!(id, self.traits.len(), "trait fixture ids should be dense");
        let (rest, super_traits) = split_top_level_keyword(rest, ": ")
            .map(|(head, tail)| (head, parse_type_bounds(tail)))
            .unwrap_or((rest, Vec::new()));
        let (name, generics) = parse_named_generics(rest);
        let trait_ref = trait_ref(id);
        self.trait_refs_by_name.insert(name.to_string(), trait_ref);
        let mut data = trait_data(id, name, generics);
        data.super_traits = super_traits;
        self.traits.push(data);
    }

    fn parse_struct(&mut self, line: &str) {
        let (id, rest) = parse_numbered_line(line, "struct#");
        assert_eq!(id, self.structs.len(), "struct fixture ids should be dense");
        let (name, generics) = parse_named_generics(rest);
        let def = type_def(id);
        self.type_refs_by_name.insert(name.to_string(), def);
        self.structs.push(struct_data(id, name, generics));
    }

    fn parse_impl(&mut self, line: &str) {
        let (id, rest) = parse_numbered_line(line, "impl#");
        assert_eq!(id, self.impls.len(), "impl fixture ids should be dense");
        let (rest, note) = split_trailing_note(rest);
        let rest = rest
            .strip_prefix("impl")
            .expect("impl fixture should start with `impl`")
            .trim();

        let (mut generics, rest) = parse_leading_generics(rest);
        let (rest, where_predicates) = split_top_level_keyword(rest, " where ")
            .map(|(head, tail)| (head, parse_where_predicates(tail)))
            .unwrap_or((rest, Vec::new()));
        generics.where_predicates.extend(where_predicates);

        let (trait_ty, self_ty) =
            split_top_level_keyword(rest, " for ").expect("impl fixture should contain ` for `");
        let trait_ty = parse_type_ref(trait_ty);
        let self_ty = parse_type_ref(self_ty);
        let trait_name =
            type_ref_path_name(&trait_ty).expect("impl trait fixture should be a trait path");
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
            local_impl: local_impl(id),
            source: dummy_source(),
            owner: module(),
            generics,
            trait_ref: Some(trait_ty),
            self_ty,
            resolved_self_ty,
            resolved_trait_ref: resolved_one(trait_ref),
            items: Vec::new(),
            is_unsafe: false,
        };
        self.impls.push(impl_data);
    }

    fn parse_type_alias(&mut self, line: &str) {
        let (id, rest) = parse_numbered_line(line, "type#");
        assert_eq!(
            id,
            self.type_aliases.len(),
            "type alias fixture ids should be dense"
        );
        let (owner_and_name, aliased_ty) = rest
            .split_once(" = ")
            .map(|(lhs, rhs)| (lhs, Some(parse_type_ref(rhs))))
            .unwrap_or((rest, None));
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
            .push(type_alias_data(name, owner, aliased_ty));
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
            return resolved_one(def);
        }

        let name = type_ref_path_name(self_ty)
            .expect("impl self type should be a type path or use a `resolved self` note");
        let def = *self
            .type_refs_by_name
            .get(&name)
            .unwrap_or_else(|| panic!("unknown impl self type `{name}`"));
        resolved_one(def)
    }
}

fn parse_numbered_line<'a>(line: &'a str, prefix: &str) -> (usize, &'a str) {
    let rest = line
        .strip_prefix(prefix)
        .unwrap_or_else(|| panic!("fixture line should start with `{prefix}`: {line}"));
    let (index, rest) = rest
        .split_once(' ')
        .unwrap_or_else(|| panic!("fixture line should have a number and body: {line}"));
    (parse_usize(index, prefix), rest.trim())
}

fn parse_usize(text: &str, context: &str) -> usize {
    text.parse::<usize>()
        .unwrap_or_else(|_| panic!("{context} should be a usize: {text}"))
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
    let generics = parse_generic_params(&text[angle_start + 1..angle_end]);
    (name, generics)
}

fn parse_leading_generics(text: &str) -> (GenericParams, &str) {
    let text = text.trim();
    if !text.starts_with('<') {
        return (GenericParams::default(), text);
    }

    let angle_end = matching_angle(text, 0);
    (
        parse_generic_params(&text[1..angle_end]),
        text[angle_end + 1..].trim(),
    )
}

fn parse_generic_params(text: &str) -> GenericParams {
    if text.trim().is_empty() {
        return GenericParams::default();
    }

    generics(
        split_top_level_commas(text)
            .into_iter()
            .map(parse_type_param_decl)
            .collect(),
    )
}

fn parse_type_param_decl(text: &str) -> TypeParamData {
    if let Some((name, bounds)) = split_top_level_keyword(text, ": ") {
        return type_param_with_bounds(name.trim(), parse_type_bounds(bounds));
    }

    type_param(text.trim())
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

fn parse_type_bounds(text: &str) -> Vec<TypeBound> {
    split_top_level(text, '+')
        .into_iter()
        .map(|bound| TypeBound::Trait(parse_type_ref(bound.trim())))
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
    if let Some(ty) = parse_bracket_ty(text) {
        return match ty {
            ParsedBracketTy::Slice(inner) => TypeRef::Slice(Box::new(parse_type_ref(inner))),
            ParsedBracketTy::Array { inner, len } => TypeRef::Array {
                inner: Box::new(parse_type_ref(inner)),
                len,
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
    options: TraitSelectionOptions,
}

enum TraitSelectionCaseKind {
    Probe(String),
    NormalizeAssoc(String),
    ChalkNormalizeAssoc(String),
}

impl TraitSelectionCase {
    pub(super) fn probe(title: &'static str, goal: impl Into<String>) -> Self {
        Self {
            title,
            kind: TraitSelectionCaseKind::Probe(goal.into()),
            options: TraitSelectionOptions::new(),
        }
    }

    pub(super) fn normalize_assoc(title: &'static str, goal: impl Into<String>) -> Self {
        Self {
            title,
            kind: TraitSelectionCaseKind::NormalizeAssoc(goal.into()),
            options: TraitSelectionOptions::new(),
        }
    }

    pub(super) fn chalk_normalize_assoc(title: &'static str, goal: impl Into<String>) -> Self {
        Self {
            title,
            kind: TraitSelectionCaseKind::ChalkNormalizeAssoc(goal.into()),
            options: TraitSelectionOptions::new(),
        }
    }

    pub(super) fn with_options(mut self, options: TraitSelectionOptions) -> Self {
        self.options = options;
        self
    }
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

struct TraitSelectionSnapshot {
    fixture: TraitSelectionFixture,
    cases: Vec<TraitSelectionCase>,
}

struct ParsedTraitQuery {
    goal: TraitGoal,
    table: InferenceTable,
    vars: Vec<NamedInferVar>,
    var_names: HashMap<String, String>,
}

struct ParsedAssocQuery {
    goal: TraitGoal,
    assoc_name: String,
    table: InferenceTable,
    vars: Vec<NamedInferVar>,
    var_names: HashMap<String, String>,
}

struct NamedInferVar {
    name: String,
    ty: Ty,
}

struct TraitSelectionQueryParser<'a> {
    fixture: &'a TraitSelectionFixture,
    table: InferenceTable,
    vars: Vec<NamedInferVar>,
    var_by_name: HashMap<String, Ty>,
}

impl<'a> TraitSelectionQueryParser<'a> {
    fn new(fixture: &'a TraitSelectionFixture) -> Self {
        Self {
            fixture,
            table: InferenceTable::new(),
            vars: Vec::new(),
            var_by_name: HashMap::new(),
        }
    }

    fn parse_goal(mut self, text: &str) -> ParsedTraitQuery {
        let (self_ty, trait_path) = split_top_level_keyword(text, ": ")
            .expect("trait query should be written as `Self: Trait<Args>`");
        let (trait_ref, args) = self.parse_trait_path(trait_path);
        let goal = TraitGoal {
            self_ty: self.parse_infer_ty(self_ty),
            trait_ref,
            args,
        };
        let var_names = self.var_name_map();
        ParsedTraitQuery {
            goal,
            table: self.table,
            vars: self.vars,
            var_names,
        }
    }

    fn parse_assoc_goal(mut self, text: &str) -> ParsedAssocQuery {
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
        let (trait_ref, args) = self.parse_trait_path(trait_path);
        let goal = TraitGoal {
            self_ty: self.parse_infer_ty(self_ty),
            trait_ref,
            args,
        };
        let var_names = self.var_name_map();
        ParsedAssocQuery {
            goal,
            assoc_name: assoc_name.to_string(),
            table: self.table,
            vars: self.vars,
            var_names,
        }
    }

    fn parse_trait_path(&mut self, text: &str) -> (TraitRef, Vec<GenericArg>) {
        let (name, args) = parse_path_head_and_args(text.trim());
        let trait_ref = self
            .fixture
            .trait_ref_by_name(name)
            .unwrap_or_else(|| panic!("query refers to unknown trait `{name}`"));
        let args = args
            .into_iter()
            .map(|arg| self.parse_infer_generic_arg(arg))
            .collect();
        (trait_ref, args)
    }

    fn parse_infer_ty(&mut self, text: &str) -> Ty {
        let text = text.trim();
        if text == "_" {
            return Ty::Unknown;
        }
        if let Some(name) = text.strip_prefix('?') {
            return self.type_var(name);
        }
        if let Some(id) = text
            .strip_prefix("{closure#")
            .and_then(|text| text.strip_suffix('}'))
        {
            return Ty::Closure(crate::ClosureTyId::new(rg_ir_model::ExprId(parse_usize(
                id,
                "closure id",
            ))));
        }
        if let Some(bounds) = text.strip_prefix("impl ") {
            return Ty::Opaque {
                bounds: split_top_level(bounds, '+')
                    .into_iter()
                    .map(|bound| self.parse_infer_opaque_bound(bound))
                    .collect(),
            };
        }
        if let Some(ty) = parse_bracket_ty(text) {
            return match ty {
                ParsedBracketTy::Slice(inner) => Ty::Slice(Box::new(self.parse_infer_ty(inner))),
                ParsedBracketTy::Array { inner, len } => Ty::Array {
                    inner: Box::new(self.parse_infer_ty(inner)),
                    len,
                },
            };
        }

        let (name, args) = parse_path_head_and_args(text);
        let def = self
            .fixture
            .type_ref_by_name(name)
            .unwrap_or_else(|| panic!("query refers to unknown type `{name}`"));
        let args = args
            .into_iter()
            .map(|arg| GenericArg::Type(Box::new(self.parse_infer_ty(arg))))
            .collect();
        nominal_infer_ty(def, args)
    }

    fn parse_infer_opaque_bound(&mut self, text: &str) -> OpaqueTraitBound {
        let (trait_ref, args) = self.parse_trait_path(text);
        OpaqueTraitBound { trait_ref, args }
    }

    fn parse_infer_generic_arg(&mut self, text: &str) -> GenericArg {
        if let Some((name, ty)) = split_top_level_keyword(text, " = ") {
            return GenericArg::AssocType {
                name: Name::new(name),
                ty: Some(Box::new(self.parse_infer_ty(ty))),
            };
        }

        GenericArg::Type(Box::new(self.parse_infer_ty(text)))
    }

    fn type_var(&mut self, name: &str) -> Ty {
        if let Some(ty) = self.var_by_name.get(name) {
            return ty.clone();
        }

        let ty = self.table.new_type_var();
        self.var_by_name.insert(name.to_string(), ty.clone());
        self.vars.push(NamedInferVar {
            name: name.to_string(),
            ty: ty.clone(),
        });
        ty
    }

    fn var_name_map(&self) -> HashMap<String, String> {
        self.vars
            .iter()
            .filter_map(|var| match &var.ty {
                Ty::InferVar { id, .. } => Some((
                    TraitSelectionSnapshot::render_debug_tuple_id(id),
                    var.name.clone(),
                )),
                _ => None,
            })
            .collect()
    }
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
        writeln!(dump, "  options: {}", Self::render_options(case.options))
            .expect("string writes should not fail");

        match &case.kind {
            TraitSelectionCaseKind::Probe(goal) => self.render_probe_case(case, goal, dump),
            TraitSelectionCaseKind::NormalizeAssoc(goal) => {
                self.render_normalize_case(case, goal, false, dump);
            }
            TraitSelectionCaseKind::ChalkNormalizeAssoc(goal) => {
                self.render_normalize_case(case, goal, true, dump);
            }
        }
    }

    fn render_probe_case(&self, case: &TraitSelectionCase, goal: &str, dump: &mut String) {
        let parsed = TraitSelectionQueryParser::new(&self.fixture).parse_goal(goal);
        writeln!(
            dump,
            "  goal: {}",
            self.render_goal(&parsed.goal, &parsed.var_names)
        )
        .expect("string writes should not fail");

        let result = query(&self.fixture)
            .with_options(case.options)
            .probe(&parsed.goal, &parsed.table)
            .expect("trait selection fixture query should not fail");

        match result {
            ExpectedUnique::Empty => {
                writeln!(dump, "  result: empty").expect("string writes should not fail");
            }
            ExpectedUnique::Ambiguous => {
                writeln!(dump, "  result: ambiguous").expect("string writes should not fail");
            }
            ExpectedUnique::One(selection) => {
                writeln!(dump, "  result: one").expect("string writes should not fail");
                writeln!(
                    dump,
                    "    impl: impl#{}",
                    selection.trait_impl.impl_ref.id.0
                )
                .expect("string writes should not fail");
                writeln!(
                    dump,
                    "    applicability: {}",
                    Self::render_applicability(selection.applicability)
                )
                .expect("string writes should not fail");
                self.render_named_vars(&parsed.vars, &selection.table, dump);
            }
        }
    }

    fn render_normalize_case(
        &self,
        case: &TraitSelectionCase,
        goal: &str,
        chalk_direct: bool,
        dump: &mut String,
    ) {
        let parsed = TraitSelectionQueryParser::new(&self.fixture).parse_assoc_goal(goal);
        writeln!(
            dump,
            "  goal: <{} as {}>::{}",
            self.render_infer_ty_with_vars(&parsed.goal.self_ty, &parsed.var_names),
            self.render_trait_path_with_vars(
                parsed.goal.trait_ref,
                &parsed.goal.args,
                &parsed.var_names
            ),
            parsed.assoc_name
        )
        .expect("string writes should not fail");

        let projection = if chalk_direct {
            let item_paths = ItemPathQuery::new(&self.fixture, &self.fixture);
            let target_items =
                TargetItemQuery::new(&self.fixture, &self.fixture, self.fixture.target);
            let mut solver = ChalkTraitSolver::new(&item_paths, &target_items)
                .expect("Chalk fixture solver should build");
            solver.normalize_assoc_type(
                &item_paths,
                TypePathContext::module(module()),
                &parsed.goal,
                &parsed.assoc_name,
                &parsed.table,
            )
        } else {
            query(&self.fixture)
                .with_options(case.options)
                .normalize_assoc_type(&parsed.goal, &parsed.assoc_name, &parsed.table)
                .expect("trait selection fixture projection should not fail")
        };

        let Some(projection) = projection else {
            writeln!(dump, "  result: none").expect("string writes should not fail");
            return;
        };

        writeln!(dump, "  result: projected").expect("string writes should not fail");
        writeln!(
            dump,
            "    infer: {}",
            self.render_infer_ty_with_vars(&projection.ty, &parsed.var_names)
        )
        .expect("string writes should not fail");
        writeln!(
            dump,
            "    final: {}",
            self.render_ty(&projection.table.finalize(&projection.ty))
        )
        .expect("string writes should not fail");
        writeln!(
            dump,
            "    applicability: {}",
            Self::render_applicability(projection.applicability)
        )
        .expect("string writes should not fail");
        self.render_named_vars(&parsed.vars, &projection.table, dump);
    }

    fn render_named_vars(&self, vars: &[NamedInferVar], table: &InferenceTable, dump: &mut String) {
        if vars.is_empty() {
            return;
        }

        writeln!(dump, "    vars").expect("string writes should not fail");
        for var in vars {
            let result = self.render_ty(&table.finalize(&var.ty));
            writeln!(dump, "      ?{} = {result}", var.name)
                .expect("string writes should not fail");
        }
    }

    fn render_goal(&self, goal: &TraitGoal, var_names: &HashMap<String, String>) -> String {
        format!(
            "{}: {}",
            self.render_infer_ty_with_vars(&goal.self_ty, var_names),
            self.render_trait_path_with_vars(goal.trait_ref, &goal.args, var_names)
        )
    }

    fn render_trait_path_with_vars(
        &self,
        trait_ref: TraitRef,
        args: &[GenericArg],
        var_names: &HashMap<String, String>,
    ) -> String {
        let name = self.render_trait_ref(trait_ref);
        if args.is_empty() {
            return name;
        }

        format!(
            "{}<{}>",
            name,
            args.iter()
                .map(|arg| self.render_infer_generic_arg_with_vars(arg, var_names))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    fn render_trait_ref(&self, trait_ref: TraitRef) -> String {
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

    fn render_infer_ty_with_vars(&self, ty: &Ty, var_names: &HashMap<String, String>) -> String {
        match ty {
            Ty::InferVar { kind, id } => match kind {
                InferVarKind::Type => Self::render_named_var("?", id, var_names),
                InferVarKind::Integer => Self::render_named_var("?int", id, var_names),
                InferVarKind::Float => Self::render_named_var("?float", id, var_names),
            },
            Ty::Unit => "()".to_string(),
            Ty::Never => "!".to_string(),
            Ty::Primitive(primitive) => Self::render_primitive(*primitive),
            Ty::Tuple(fields) => self.render_infer_tuple_with_vars(fields, var_names),
            Ty::Array { inner, len } => {
                format!(
                    "[{}; {}]",
                    self.render_infer_ty_with_vars(inner, var_names),
                    len.as_deref().unwrap_or("_")
                )
            }
            Ty::Slice(inner) => {
                format!("[{}]", self.render_infer_ty_with_vars(inner, var_names))
            }
            Ty::Reference { mutability, inner } => {
                format!(
                    "{}{}",
                    mutability.render_prefix(),
                    self.render_infer_ty_with_vars(inner, var_names)
                )
            }
            Ty::Opaque { bounds } => {
                let bounds = bounds
                    .iter()
                    .map(|bound| self.render_infer_opaque_bound_with_vars(bound, var_names))
                    .collect::<Vec<_>>()
                    .join(" + ");
                format!("impl {bounds}")
            }
            Ty::Closure(id) => format!("{{closure#{id}}}"),
            Ty::FunctionItem(function) => format!("{{fn-item:{function:?}}}"),
            Ty::Syntax(ty) => ty.to_string(),
            Ty::Nominal(ty) => self.render_infer_nominal_ty_with_vars(ty, var_names),
            Ty::SelfTy(ty) => {
                format!(
                    "Self({})",
                    self.render_infer_nominal_ty_with_vars(ty, var_names)
                )
            }
            Ty::Unknown => "_".to_string(),
        }
    }

    fn render_named_var(
        fallback_prefix: &str,
        id: &impl Debug,
        var_names: &HashMap<String, String>,
    ) -> String {
        let id = Self::render_debug_tuple_id(id);
        if let Some(name) = var_names.get(&id) {
            return format!("?{name}");
        }

        format!("{fallback_prefix}{id}")
    }

    fn render_infer_tuple_with_vars(
        &self,
        fields: &[Ty],
        var_names: &HashMap<String, String>,
    ) -> String {
        if fields.is_empty() {
            return "()".to_string();
        }

        let mut rendered = fields
            .iter()
            .map(|field| self.render_infer_ty_with_vars(field, var_names))
            .collect::<Vec<_>>();
        if rendered.len() == 1 {
            rendered[0].push(',');
        }

        format!("({})", rendered.join(", "))
    }

    fn render_ty(&self, ty: &Ty) -> String {
        match ty {
            Ty::Unit => "()".to_string(),
            Ty::Never => "!".to_string(),
            Ty::Primitive(primitive) => Self::render_primitive(*primitive),
            Ty::Tuple(fields) => self.render_tuple(fields, Self::render_ty),
            Ty::Array { inner, len } => {
                format!(
                    "[{}; {}]",
                    self.render_ty(inner),
                    len.as_deref().unwrap_or("_")
                )
            }
            Ty::Slice(inner) => format!("[{}]", self.render_ty(inner)),
            Ty::Reference { mutability, inner } => {
                format!("{}{}", mutability.render_prefix(), self.render_ty(inner))
            }
            Ty::Opaque { bounds } => {
                let bounds = bounds
                    .iter()
                    .map(|bound| self.render_opaque_bound(bound))
                    .collect::<Vec<_>>()
                    .join(" + ");
                format!("impl {bounds}")
            }
            Ty::Closure(id) => format!("{{closure#{id}}}"),
            Ty::FunctionItem(function) => format!("{{fn-item:{function:?}}}"),
            Ty::Syntax(ty) => ty.to_string(),
            Ty::Nominal(ty) => self.render_nominal_ty(ty),
            Ty::SelfTy(ty) => format!("Self({})", self.render_nominal_ty(ty)),
            Ty::InferVar { kind, id } => match kind {
                InferVarKind::Type => Self::render_named_var("?", id, &HashMap::new()),
                InferVarKind::Integer => Self::render_named_var("?int", id, &HashMap::new()),
                InferVarKind::Float => Self::render_named_var("?float", id, &HashMap::new()),
            },
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

    fn render_infer_nominal_ty_with_vars(
        &self,
        ty: &NominalTy,
        var_names: &HashMap<String, String>,
    ) -> String {
        let name = self.render_type_def_ref(ty.def);
        if ty.args.is_empty() {
            return name;
        }

        format!(
            "{}<{}>",
            name,
            ty.args
                .iter()
                .map(|arg| self.render_infer_generic_arg_with_vars(arg, var_names))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    fn render_nominal_ty(&self, ty: &NominalTy) -> String {
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

    fn render_infer_generic_arg_with_vars(
        &self,
        arg: &GenericArg,
        var_names: &HashMap<String, String>,
    ) -> String {
        match arg {
            GenericArg::Type(ty) => self.render_infer_ty_with_vars(ty, var_names),
            GenericArg::Lifetime(lifetime) => lifetime.to_string(),
            GenericArg::Const(value) => value.clone(),
            GenericArg::FnTraitArgs { params, ret } => {
                let params = params
                    .iter()
                    .map(|param| self.render_infer_ty_with_vars(param, var_names))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "({params}) -> {}",
                    self.render_infer_ty_with_vars(ret, var_names)
                )
            }
            GenericArg::AssocType { name, ty } => match ty {
                Some(ty) => format!("{name} = {}", self.render_infer_ty_with_vars(ty, var_names)),
                None => name.to_string(),
            },
            GenericArg::Unsupported(text) => format!("<unsupported:{text}>"),
        }
    }

    fn render_generic_arg(&self, arg: &GenericArg) -> String {
        match arg {
            GenericArg::Type(ty) => self.render_ty(ty),
            GenericArg::Lifetime(lifetime) => lifetime.to_string(),
            GenericArg::Const(value) => value.clone(),
            GenericArg::FnTraitArgs { params, ret } => {
                let params = params
                    .iter()
                    .map(|param| self.render_ty(param))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({params}) -> {}", self.render_ty(ret))
            }
            GenericArg::AssocType { name, ty } => match ty {
                Some(ty) => format!("{name} = {}", self.render_ty(ty)),
                None => name.to_string(),
            },
            GenericArg::Unsupported(text) => format!("<unsupported:{text}>"),
        }
    }

    fn render_infer_opaque_bound_with_vars(
        &self,
        bound: &OpaqueTraitBound,
        var_names: &HashMap<String, String>,
    ) -> String {
        self.render_trait_path_with_vars(bound.trait_ref, &bound.args, var_names)
    }

    fn render_opaque_bound(&self, bound: &OpaqueTraitBound) -> String {
        let name = self.render_trait_ref(bound.trait_ref);
        if bound.args.is_empty() {
            return name;
        }

        format!(
            "{}<{}>",
            name,
            bound
                .args
                .iter()
                .map(|arg| self.render_generic_arg(arg))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    fn render_options(options: TraitSelectionOptions) -> &'static str {
        if options == TraitSelectionOptions::new() {
            "default"
        } else if options == TraitSelectionOptions::new().header_only() {
            "header-only"
        } else if options == TraitSelectionOptions::new().caller_solves_impl_predicates() {
            "caller-solves-impl-predicates"
        } else if options == TraitSelectionOptions::new().keep_maybe_candidates() {
            "keep-maybe-candidates"
        } else {
            "custom"
        }
    }

    fn render_applicability(applicability: TraitApplicability) -> &'static str {
        match applicability {
            TraitApplicability::Yes => "yes",
            TraitApplicability::Maybe => "maybe",
            TraitApplicability::No => "no",
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

    fn render_debug_tuple_id(id: &impl Debug) -> String {
        let text = format!("{id:?}");
        text.strip_prefix("InferVarId(")
            .and_then(|text| text.strip_suffix(')'))
            .unwrap_or(&text)
            .to_string()
    }
}
