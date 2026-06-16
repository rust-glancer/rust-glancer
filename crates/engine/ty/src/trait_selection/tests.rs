use std::convert::Infallible;

use rg_ir_model::hir::items::ImplData;
use rg_ir_model::hir::source::{GeneratedItemRef, GeneratedSourceId, ItemSource, ItemSourceKind};
use rg_ir_model::items::{
    GenericArg as ItemGenericArg, GenericParams, ItemTreeId, TypeBound, TypeParamData, TypePath,
    TypePathSegment, TypeRef,
};
use rg_ir_model::{
    DefMapRef, ImplId, LocalImplId, LocalImplRef, ModuleId, ModuleRef, PackageSlot, StructId,
    TargetId, TargetRef, TraitId, TraitImplRef, TraitRef, TypeDefId, TypeDefRef,
};
use rg_ir_storage::{DefMap, DefMapSource, ItemStore, ItemStoreBuilder, ItemStoreSource};
use rg_std::ExpectedUnique;
use rg_text::Name;

use super::{TraitGoal, TraitSelectionQuery};
use crate::inference::{InferGenericArg, InferNominalTy, InferTy, InferenceTable};
use crate::{GenericArg, ItemPathQuery, NominalTy, Ty};

struct TraitSelectionFixture {
    store: ItemStore,
    target: TargetRef,
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
        // Source coordinates are irrelevant for trait-selection tests. These IDs are simple
        // integer newtypes in `rg_parse`, but `rg_ty` does not depend on that crate directly.
        file_id: unsafe { std::mem::zeroed() },
        kind: ItemSourceKind::Generated(GeneratedItemRef {
            source: GeneratedSourceId(0),
            item: ItemTreeId(0),
        }),
    }
}

fn path_ty(name: &str, args: Vec<ItemGenericArg>) -> TypeRef {
    TypeRef::Path(TypePath {
        source_span: unsafe { std::mem::zeroed() },
        absolute: false,
        segments: vec![TypePathSegment {
            name: Name::new(name),
            args,
            span: unsafe { std::mem::zeroed() },
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

fn bounded_type_param(name: &str) -> TypeParamData {
    TypeParamData {
        name: Name::new(name),
        bounds: vec![TypeBound::Trait(path_ty("Clone", Vec::new()))],
        default: None,
    }
}

fn generics(types: Vec<TypeParamData>) -> GenericParams {
    GenericParams {
        types,
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

fn fixture(impls: Vec<ImplData>) -> TraitSelectionFixture {
    let mut builder = ItemStoreBuilder::new(origin(), 0);
    for impl_data in impls {
        builder.impls.alloc(impl_data);
    }
    TraitSelectionFixture {
        store: builder.build(),
        target: target(),
    }
}

fn query(
    fixture: &TraitSelectionFixture,
) -> TraitSelectionQuery<'_, &TraitSelectionFixture, &TraitSelectionFixture> {
    TraitSelectionQuery::new(
        ItemPathQuery::new(fixture, fixture),
        rg_ir_storage::TargetItemQuery::new(fixture, fixture, fixture.target),
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
fn probe_skips_impls_that_need_bound_solving() {
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
