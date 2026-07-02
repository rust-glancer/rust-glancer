use std::convert::Infallible;

use rg_ir_model::hir::items::{ImplData, TraitData};
use rg_ir_model::hir::source::{GeneratedItemRef, GeneratedSourceId, ItemSource, ItemSourceKind};
use rg_ir_model::items::{
    GenericArg as ItemGenericArg, GenericParams, ItemTreeId, TypeBound, TypeParamData, TypePath,
    TypePathSegment, TypeRef, VisibilityLevel, WherePredicate,
};
use rg_ir_model::{
    DefMapRef, ExprId, FileId, ImplId, LocalDefId, LocalDefRef, LocalImplId, LocalImplRef,
    ModuleId, ModuleRef, PackageSlot, Span, StructId, TargetId, TargetRef, TextSpan, TraitId,
    TraitImplRef, TraitRef, TypeDefId, TypeDefRef,
};
use rg_ir_storage::{
    DefMap, DefMapSource, ItemLookupIndex, ItemStore, ItemStoreBuilder, ItemStoreSource,
    TargetItemQuery,
};
use rg_std::ExpectedUnique;
use rg_text::Name;

use super::{TraitGoal, TraitSelectionOptions, TraitSelectionQuery};
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
    ImplData {
        local_impl: local_impl(index),
        source: dummy_source(),
        owner: module(),
        generics,
        trait_ref: Some(trait_ty),
        self_ty,
        resolved_self_ty: resolved_one(self_def),
        resolved_trait_ref: resolved_one(trait_ref),
        items: Vec::new(),
        is_unsafe: false,
    }
}

fn trait_data(index: usize, name: &str, generics: GenericParams) -> TraitData {
    TraitData {
        local_def: local_def(index),
        source: dummy_source(),
        owner: module(),
        name: Name::new(name),
        visibility: VisibilityLevel::Public,
        docs: None,
        generics,
        super_traits: Vec::new(),
        items: Vec::new(),
        is_unsafe: false,
    }
}

fn fixture(impls: Vec<ImplData>) -> TraitSelectionFixture {
    fixture_with_traits(Vec::new(), impls)
}

fn fixture_with_traits(traits: Vec<TraitData>, impls: Vec<ImplData>) -> TraitSelectionFixture {
    let mut builder = ItemStoreBuilder::new(origin(), 0);
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
