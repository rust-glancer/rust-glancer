use std::convert::Infallible;

use rg_ir_model::hir::items::{ImplData, StructData, TraitData, TypeAliasData};
use rg_ir_model::hir::signature::TypeAliasSignature;
use rg_ir_model::hir::source::{GeneratedItemRef, GeneratedSourceId, ItemSource, ItemSourceKind};
use rg_ir_model::items::{
    FieldList, GenericArg as ItemGenericArg, GenericParams, ItemTreeId, TypeAliasItem, TypeBound,
    TypeParamData, TypePath, TypePathSegment, TypeRef, VisibilityLevel, WherePredicate,
};
use rg_ir_model::{
    AssocItemId, DefMapRef, ExprId, FileId, ImplId, ItemOwner, LocalDefId, LocalDefRef,
    LocalImplId, LocalImplRef, ModuleId, ModuleRef, PackageSlot, Span, StructId, TargetId,
    TargetRef, TextSpan, TraitApplicability, TraitId, TraitImplRef, TraitRef, TypeAliasId,
    TypeDefId, TypeDefRef,
};
use rg_ir_storage::{
    DefMap, DefMapSource, ItemLookupIndex, ItemStore, ItemStoreBuilder, ItemStoreSource,
    TargetItemQuery, TypePathContext,
};
use rg_std::ExpectedUnique;
use rg_text::Name;

use super::{ChalkTraitSolver, TraitGoal, TraitSelectionOptions, TraitSelectionQuery};
use crate::inference::{InferGenericArg, InferNominalTy, InferTy, InferenceTable};
use crate::{ClosureTyId, GenericArg, ItemPathQuery, NominalTy, Ty};

struct TraitSelectionFixture {
    store: ItemStore,
    target: TargetRef,
    lookup_index: ItemLookupIndex,
}

impl DefMapSource for TraitSelectionFixture {
    type Error = Infallible;

    fn def_map_for_origin(&self, _origin: DefMapRef) -> Result<Option<&DefMap>, Self::Error> {
        Ok(None)
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

fn target() -> TargetRef {
    TargetRef {
        package: PackageSlot(0),
        target: TargetId(0),
    }
}

fn origin() -> DefMapRef {
    DefMapRef::Target(target())
}

fn module() -> ModuleRef {
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

fn type_def(index: usize) -> TypeDefRef {
    TypeDefRef {
        origin: origin(),
        id: TypeDefId::Struct(StructId(index)),
    }
}

fn trait_ref(index: usize) -> TraitRef {
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

fn trait_impl(index: usize, trait_ref: TraitRef) -> TraitImplRef {
    TraitImplRef {
        impl_ref: rg_ir_model::ImplRef {
            origin: origin(),
            id: ImplId(index),
        },
        trait_ref,
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

fn path_ty(name: &str, args: Vec<ItemGenericArg>) -> TypeRef {
    let span = Span {
        text: TextSpan { start: 0, end: 0 },
    };

    TypeRef::Path(TypePath {
        source_span: span,
        absolute: false,
        segments: vec![TypePathSegment {
            name: Name::new(name),
            args,
            span,
        }],
    })
}

fn type_arg(ty: TypeRef) -> ItemGenericArg {
    ItemGenericArg::Type(ty)
}

fn type_param(name: &str) -> TypeParamData {
    TypeParamData {
        name: Name::new(name),
        bounds: Vec::new(),
        default: None,
    }
}

fn type_param_with_bounds(name: &str, bounds: Vec<TypeBound>) -> TypeParamData {
    TypeParamData {
        name: Name::new(name),
        bounds,
        default: None,
    }
}

fn bounded_type_param(name: &str) -> TypeParamData {
    type_param_with_bounds(name, vec![TypeBound::Trait(path_ty("Clone", Vec::new()))])
}

fn generics(types: Vec<TypeParamData>) -> GenericParams {
    GenericParams {
        types,
        ..GenericParams::default()
    }
}

fn generics_with_where(
    types: Vec<TypeParamData>,
    where_predicates: Vec<WherePredicate>,
) -> GenericParams {
    GenericParams {
        types,
        where_predicates,
        ..GenericParams::default()
    }
}

fn nominal_infer_ty(def: TypeDefRef, args: Vec<InferGenericArg>) -> InferTy {
    InferTy::Nominal(InferNominalTy { def, args })
}

fn nominal_ty(def: TypeDefRef) -> Ty {
    Ty::nominal(NominalTy::bare(def))
}

fn infer_type_arg(ty: InferTy) -> InferGenericArg {
    InferGenericArg::Type(Box::new(ty))
}

fn resolved_one<T: PartialEq>(value: T) -> ExpectedUnique<T> {
    let mut resolved = ExpectedUnique::new();
    resolved.push(value);
    resolved
}

fn impl_data(
    index: usize,
    generics: GenericParams,
    trait_ref: TraitRef,
    trait_ty: TypeRef,
    self_def: TypeDefRef,
    self_ty: TypeRef,
) -> ImplData {
    impl_data_with_items(
        index,
        generics,
        trait_ref,
        trait_ty,
        self_def,
        self_ty,
        Vec::new(),
    )
}

fn impl_data_with_items(
    index: usize,
    generics: GenericParams,
    trait_ref: TraitRef,
    trait_ty: TypeRef,
    self_def: TypeDefRef,
    self_ty: TypeRef,
    items: Vec<AssocItemId>,
) -> ImplData {
    ImplData {
        local_impl: local_impl(index),
        source: dummy_source(),
        owner: module(),
        generics,
        trait_ref: Some(trait_ty),
        self_ty,
        resolved_self_ty: resolved_one(self_def),
        resolved_trait_ref: resolved_one(trait_ref),
        items,
        is_unsafe: false,
    }
}

fn trait_data(index: usize, name: &str, generics: GenericParams) -> TraitData {
    trait_data_with_items(index, name, generics, Vec::new())
}

fn trait_data_with_items(
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

fn struct_data(index: usize, name: &str, generics: GenericParams) -> StructData {
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

fn type_alias_data(name: &str, owner: ItemOwner, aliased_ty: Option<TypeRef>) -> TypeAliasData {
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

fn fixture(impls: Vec<ImplData>) -> TraitSelectionFixture {
    fixture_with_traits(Vec::new(), impls)
}

fn fixture_with_traits(traits: Vec<TraitData>, impls: Vec<ImplData>) -> TraitSelectionFixture {
    fixture_with_traits_impls_and_aliases(traits, impls, Vec::new())
}

fn fixture_with_traits_impls_and_aliases(
    traits: Vec<TraitData>,
    impls: Vec<ImplData>,
    type_aliases: Vec<TypeAliasData>,
) -> TraitSelectionFixture {
    fixture_with_traits_impls_aliases_and_structs(traits, impls, type_aliases, Vec::new())
}

fn fixture_with_traits_impls_aliases_and_structs(
    traits: Vec<TraitData>,
    impls: Vec<ImplData>,
    type_aliases: Vec<TypeAliasData>,
    structs: Vec<StructData>,
) -> TraitSelectionFixture {
    let mut builder = ItemStoreBuilder::new(origin(), 0);
    for struct_data in structs {
        builder.structs.alloc(struct_data);
    }
    for type_alias_data in type_aliases {
        builder.type_aliases.alloc(type_alias_data);
    }
    for trait_data in traits {
        builder.traits.alloc(trait_data);
    }
    for impl_data in impls {
        builder.impls.alloc(impl_data);
    }
    let mut fixture = TraitSelectionFixture {
        store: builder.build(),
        target: target(),
        lookup_index: ItemLookupIndex::default(),
    };
    {
        let target_items = TargetItemQuery::new(&fixture, &fixture, fixture.target);
        fixture.lookup_index =
            ItemLookupIndex::build_from(&target_items).expect("fixture lookup index should build");
    }
    fixture
}

fn query(
    fixture: &TraitSelectionFixture,
) -> TraitSelectionQuery<'_, &TraitSelectionFixture, &TraitSelectionFixture> {
    TraitSelectionQuery::with_index(
        ItemPathQuery::new(fixture, fixture),
        TargetItemQuery::new(fixture, fixture, fixture.target),
        &fixture.lookup_index,
    )
}

fn generic_iterator_assoc_fixture() -> (TraitSelectionFixture, TypeDefRef, TypeDefRef, TraitRef) {
    let iter_def = type_def(0);
    let user_def = type_def(1);
    let iterator = trait_ref(0);
    let trait_item = TypeAliasId(0);
    let impl_item = TypeAliasId(1);

    let iterator_data = trait_data_with_items(
        0,
        "Iterator",
        GenericParams::default(),
        vec![AssocItemId::TypeAlias(trait_item)],
    );
    let iter_impl = impl_data_with_items(
        0,
        generics(vec![type_param("T")]),
        iterator,
        path_ty("Iterator", Vec::new()),
        iter_def,
        path_ty("Iter", vec![type_arg(path_ty("T", Vec::new()))]),
        vec![AssocItemId::TypeAlias(impl_item)],
    );
    let fixture = fixture_with_traits_impls_aliases_and_structs(
        vec![iterator_data],
        vec![iter_impl],
        vec![
            type_alias_data("Item", ItemOwner::Trait(TraitId(0)), None),
            type_alias_data(
                "Item",
                ItemOwner::Impl(ImplId(0)),
                Some(path_ty("T", Vec::new())),
            ),
        ],
        vec![
            struct_data(0, "Iter", generics(vec![type_param("T")])),
            struct_data(1, "User", GenericParams::default()),
        ],
    );

    (fixture, iter_def, user_def, iterator)
}

#[test]
fn probe_selects_direct_from_iterator_impl_and_solves_destination_arg() {
    let vec_def = type_def(0);
    let user_def = type_def(1);
    let from_iterator = trait_ref(0);
    let impl_data = impl_data(
        0,
        generics(vec![type_param("T")]),
        from_iterator,
        path_ty("FromIterator", vec![type_arg(path_ty("T", Vec::new()))]),
        vec_def,
        path_ty("Vec", vec![type_arg(path_ty("T", Vec::new()))]),
    );
    let fixture = fixture(vec![impl_data]);

    let mut table = InferenceTable::new();
    let element = table.new_type_var();
    let goal_self = nominal_infer_ty(vec_def, vec![infer_type_arg(element.clone())]);
    let goal = TraitGoal {
        self_ty: goal_self.clone(),
        trait_ref: from_iterator,
        args: vec![infer_type_arg(InferTy::from_ty(&nominal_ty(user_def)))],
    };

    let selection = query(&fixture).probe(&goal, &table).unwrap();
    let ExpectedUnique::One(selection) = selection else {
        panic!("direct impl should be selected uniquely");
    };

    assert_eq!(selection.trait_impl, trait_impl(0, from_iterator));
    assert_eq!(
        selection.table.finalize(&goal_self),
        Ty::nominal(NominalTy {
            def: vec_def,
            args: vec![GenericArg::Type(Box::new(nominal_ty(user_def)))],
        })
    );
    assert_eq!(
        table.finalize(&goal_self),
        Ty::nominal(NominalTy {
            def: vec_def,
            args: vec![GenericArg::Type(Box::new(Ty::Unknown))],
        })
    );
}

#[test]
fn normalize_assoc_type_projects_generic_impl_value() {
    let (fixture, iter_def, user_def, iterator) = generic_iterator_assoc_fixture();

    let table = InferenceTable::new();
    let goal = TraitGoal {
        self_ty: nominal_infer_ty(
            iter_def,
            vec![infer_type_arg(InferTy::from_ty(&nominal_ty(user_def)))],
        ),
        trait_ref: iterator,
        args: Vec::new(),
    };

    let projection = query(&fixture)
        .normalize_assoc_type(&goal, "Item", &table)
        .unwrap()
        .expect("Iterator::Item should project through the generic impl value");

    assert_eq!(
        projection.table.finalize(&projection.ty),
        nominal_ty(user_def)
    );
}

#[test]
fn chalk_solver_normalizes_generic_associated_type_value() {
    let (fixture, iter_def, user_def, iterator) = generic_iterator_assoc_fixture();

    let table = InferenceTable::new();
    let goal = TraitGoal {
        self_ty: nominal_infer_ty(
            iter_def,
            vec![infer_type_arg(InferTy::from_ty(&nominal_ty(user_def)))],
        ),
        trait_ref: iterator,
        args: Vec::new(),
    };
    let item_paths = ItemPathQuery::new(&fixture, &fixture);
    let target_items = TargetItemQuery::new(&fixture, &fixture, fixture.target);
    let solver = ChalkTraitSolver::new(&item_paths, &target_items).unwrap();

    let (projection_ty, applicability) = solver
        .normalize_assoc_type(
            &item_paths,
            TypePathContext::module(module()),
            &goal,
            "Item",
            &table,
        )
        .expect("Chalk should normalize the generic impl associated type value");

    assert_eq!(table.finalize(&projection_ty), nominal_ty(user_def));
    assert_eq!(applicability, TraitApplicability::Yes);
}

#[test]
fn probe_rejects_concrete_self_mismatch() {
    let vec_def = type_def(0);
    let other_vec_def = type_def(1);
    let user_def = type_def(2);
    let from_iterator = trait_ref(0);
    let impl_data = impl_data(
        0,
        generics(vec![type_param("T")]),
        from_iterator,
        path_ty("FromIterator", vec![type_arg(path_ty("T", Vec::new()))]),
        vec_def,
        path_ty("Vec", vec![type_arg(path_ty("T", Vec::new()))]),
    );
    let fixture = fixture(vec![impl_data]);

    let mut table = InferenceTable::new();
    let element = table.new_type_var();
    let goal = TraitGoal {
        self_ty: nominal_infer_ty(other_vec_def, vec![infer_type_arg(element)]),
        trait_ref: from_iterator,
        args: vec![infer_type_arg(InferTy::from_ty(&nominal_ty(user_def)))],
    };

    let selection = query(&fixture).probe(&goal, &table).unwrap();

    assert!(selection.is_empty());
}

#[test]
fn probe_rejects_conflicting_repeated_type_param_evidence() {
    let vec_def = type_def(0);
    let user_def = type_def(1);
    let other_def = type_def(2);
    let from_iterator = trait_ref(0);
    let impl_data = impl_data(
        0,
        generics(vec![type_param("T")]),
        from_iterator,
        path_ty("FromIterator", vec![type_arg(path_ty("T", Vec::new()))]),
        vec_def,
        path_ty("Vec", vec![type_arg(path_ty("T", Vec::new()))]),
    );
    let fixture = fixture(vec![impl_data]);

    let table = InferenceTable::new();
    let goal = TraitGoal {
        self_ty: nominal_infer_ty(
            vec_def,
            vec![infer_type_arg(InferTy::from_ty(&nominal_ty(user_def)))],
        ),
        trait_ref: from_iterator,
        args: vec![infer_type_arg(InferTy::from_ty(&nominal_ty(other_def)))],
    };

    let selection = query(&fixture).probe(&goal, &table).unwrap();

    assert!(selection.is_empty());
}

#[test]
fn probe_keeps_multiple_applicable_impls_as_separate_candidates() {
    let vec_def = type_def(0);
    let user_def = type_def(1);
    let from_iterator = trait_ref(0);
    let make_impl = |index| {
        impl_data(
            index,
            generics(vec![type_param("T")]),
            from_iterator,
            path_ty("FromIterator", vec![type_arg(path_ty("T", Vec::new()))]),
            vec_def,
            path_ty("Vec", vec![type_arg(path_ty("T", Vec::new()))]),
        )
    };
    let fixture = fixture(vec![make_impl(0), make_impl(1)]);

    let mut table = InferenceTable::new();
    let element = table.new_type_var();
    let goal = TraitGoal {
        self_ty: nominal_infer_ty(vec_def, vec![infer_type_arg(element)]),
        trait_ref: from_iterator,
        args: vec![infer_type_arg(InferTy::from_ty(&nominal_ty(user_def)))],
    };

    let selection = query(&fixture).probe(&goal, &table).unwrap();

    assert!(selection.is_ambiguous());
}

#[test]
fn probe_rejects_impls_with_unproven_bounds() {
    let vec_def = type_def(0);
    let user_def = type_def(1);
    let from_iterator = trait_ref(0);
    let impl_data = impl_data(
        0,
        generics(vec![bounded_type_param("T")]),
        from_iterator,
        path_ty("FromIterator", vec![type_arg(path_ty("T", Vec::new()))]),
        vec_def,
        path_ty("Vec", vec![type_arg(path_ty("T", Vec::new()))]),
    );
    let fixture = fixture(vec![impl_data]);

    let mut table = InferenceTable::new();
    let element = table.new_type_var();
    let goal = TraitGoal {
        self_ty: nominal_infer_ty(vec_def, vec![infer_type_arg(element)]),
        trait_ref: from_iterator,
        args: vec![infer_type_arg(InferTy::from_ty(&nominal_ty(user_def)))],
    };

    let selection = query(&fixture).probe(&goal, &table).unwrap();

    assert!(selection.is_empty());
}

#[test]
fn probe_uses_chalk_to_prove_impl_type_param_bounds() {
    let clone = trait_ref(0);
    let from_iterator = trait_ref(1);
    let vec_def = type_def(0);
    let user_def = type_def(1);

    let from_iterator_impl = impl_data(
        0,
        generics(vec![bounded_type_param("T")]),
        from_iterator,
        path_ty("FromIterator", vec![type_arg(path_ty("T", Vec::new()))]),
        vec_def,
        path_ty("Vec", vec![type_arg(path_ty("T", Vec::new()))]),
    );
    let clone_impl = impl_data(
        1,
        GenericParams::default(),
        clone,
        path_ty("Clone", Vec::new()),
        user_def,
        path_ty("User", Vec::new()),
    );
    let fixture = fixture_with_traits(
        vec![trait_data(0, "Clone", GenericParams::default())],
        vec![from_iterator_impl, clone_impl],
    );

    let mut table = InferenceTable::new();
    let element = table.new_type_var();
    let goal_self = nominal_infer_ty(vec_def, vec![infer_type_arg(element.clone())]);
    let goal = TraitGoal {
        self_ty: goal_self.clone(),
        trait_ref: from_iterator,
        args: vec![infer_type_arg(InferTy::from_ty(&nominal_ty(user_def)))],
    };

    let selection = query(&fixture).probe(&goal, &table).unwrap();
    let ExpectedUnique::One(selection) = selection else {
        panic!("Chalk should prove the Clone bound through the visible Clone impl");
    };

    assert_eq!(selection.trait_impl, trait_impl(0, from_iterator));
    assert_eq!(
        selection.table.finalize(&goal_self),
        Ty::nominal(NominalTy {
            def: vec_def,
            args: vec![GenericArg::Type(Box::new(nominal_ty(user_def)))],
        })
    );
}

#[test]
fn probe_handles_visible_trait_data_with_generic_bounds() {
    let clone = trait_ref(0);
    let needs_clone = trait_ref(1);
    let from_iterator = trait_ref(2);
    let vec_def = type_def(0);
    let user_def = type_def(1);

    let from_iterator_impl = impl_data(
        0,
        generics(vec![type_param_with_bounds(
            "T",
            vec![TypeBound::Trait(path_ty(
                "NeedsClone",
                vec![type_arg(path_ty("T", Vec::new()))],
            ))],
        )]),
        from_iterator,
        path_ty("FromIterator", vec![type_arg(path_ty("T", Vec::new()))]),
        vec_def,
        path_ty("Vec", vec![type_arg(path_ty("T", Vec::new()))]),
    );
    let needs_clone_impl = impl_data(
        1,
        generics(vec![bounded_type_param("T")]),
        needs_clone,
        path_ty("NeedsClone", vec![type_arg(path_ty("T", Vec::new()))]),
        user_def,
        path_ty("T", Vec::new()),
    );
    let clone_impl = impl_data(
        2,
        GenericParams::default(),
        clone,
        path_ty("Clone", Vec::new()),
        user_def,
        path_ty("User", Vec::new()),
    );
    let fixture = fixture_with_traits(
        vec![
            trait_data(0, "Clone", GenericParams::default()),
            trait_data(1, "NeedsClone", generics(vec![bounded_type_param("T")])),
        ],
        vec![from_iterator_impl, needs_clone_impl, clone_impl],
    );

    let mut table = InferenceTable::new();
    let element = table.new_type_var();
    let goal = TraitGoal {
        self_ty: nominal_infer_ty(vec_def, vec![infer_type_arg(element)]),
        trait_ref: from_iterator,
        args: vec![infer_type_arg(InferTy::from_ty(&nominal_ty(user_def)))],
    };

    assert!(
        matches!(
            query(&fixture).probe(&goal, &table).unwrap(),
            ExpectedUnique::One(selection) if selection.trait_impl == trait_impl(0, from_iterator)
        ),
        "Chalk should prove bounds even when a visible trait datum has its own generic bounds"
    );
}

#[test]
fn caller_solved_where_predicates_can_return_impl_that_still_needs_solving() {
    let adapter_def = type_def(0);
    let produces = trait_ref(0);
    let impl_data = impl_data(
        0,
        generics_with_where(
            vec![type_param("F"), type_param("R")],
            vec![WherePredicate::Type {
                ty: path_ty("F", Vec::new()),
                bounds: vec![TypeBound::Trait(path_ty("FnOnce", Vec::new()))],
            }],
        ),
        produces,
        path_ty("Produces", Vec::new()),
        adapter_def,
        path_ty("Adapter", vec![type_arg(path_ty("F", Vec::new()))]),
    );
    let fixture = fixture(vec![impl_data]);

    let goal = TraitGoal {
        self_ty: nominal_infer_ty(
            adapter_def,
            vec![infer_type_arg(InferTy::Closure(ClosureTyId::new(ExprId(
                0,
            ))))],
        ),
        trait_ref: produces,
        args: Vec::new(),
    };
    let table = InferenceTable::new();

    assert!(query(&fixture).probe(&goal, &table).unwrap().is_empty());
    assert!(
        matches!(
            query(&fixture)
                .with_options(TraitSelectionOptions::new().caller_solves_where_predicates())
                .probe(&goal, &table)
                .unwrap(),
            ExpectedUnique::One(_)
        ),
        "caller-solved where predicate mode should leave the predicate to a higher layer"
    );
}

#[test]
fn header_only_rejects_impl_type_param_bounds_even_when_chalk_can_prove_them() {
    let clone = trait_ref(0);
    let from_iterator = trait_ref(1);
    let vec_def = type_def(0);
    let user_def = type_def(1);

    let from_iterator_impl = impl_data(
        0,
        generics(vec![bounded_type_param("T")]),
        from_iterator,
        path_ty("FromIterator", vec![type_arg(path_ty("T", Vec::new()))]),
        vec_def,
        path_ty("Vec", vec![type_arg(path_ty("T", Vec::new()))]),
    );
    let clone_impl = impl_data(
        1,
        GenericParams::default(),
        clone,
        path_ty("Clone", Vec::new()),
        user_def,
        path_ty("User", Vec::new()),
    );
    let fixture = fixture_with_traits(
        vec![trait_data(0, "Clone", GenericParams::default())],
        vec![from_iterator_impl, clone_impl],
    );

    let mut table = InferenceTable::new();
    let element = table.new_type_var();
    let goal = TraitGoal {
        self_ty: nominal_infer_ty(vec_def, vec![infer_type_arg(element)]),
        trait_ref: from_iterator,
        args: vec![infer_type_arg(InferTy::from_ty(&nominal_ty(user_def)))],
    };

    assert!(
        matches!(
            query(&fixture).probe(&goal, &table).unwrap(),
            ExpectedUnique::One(_)
        ),
        "default selection should let Chalk prove the Clone bound"
    );
    assert!(
        query(&fixture)
            .with_options(TraitSelectionOptions::new().header_only())
            .probe(&goal, &table)
            .unwrap()
            .is_empty(),
        "header-only selection must not pretend generic parameter bounds are true"
    );
}
