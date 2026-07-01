use rg_ir_model::{
    DefMapRef, ExprId, PackageSlot, StructId, TargetId, TargetRef, TraitId, TraitRef, TypeDefId,
    TypeDefRef,
    items::{FloatTy, SignedIntTy, TypeRef, UnsignedIntTy},
};

use super::{InferTy, InferenceTable};
use crate::{
    ClosureTyId, GenericArg, NominalTy, PrimitiveTy, Ty,
    inference::{
        ExplicitTypeArgInstantiationBuilder, InferGenericArg, InferNominalTy,
        InferOpaqueTraitBound, UnknownTypeInstantiationBuilder,
    },
};

fn def_map_ref() -> DefMapRef {
    DefMapRef::Target(TargetRef {
        package: PackageSlot(0),
        target: TargetId(0),
    })
}

fn type_def(index: usize) -> TypeDefRef {
    TypeDefRef {
        origin: def_map_ref(),
        id: TypeDefId::Struct(StructId(index)),
    }
}

fn trait_ref(index: usize) -> TraitRef {
    TraitRef {
        origin: def_map_ref(),
        id: TraitId(index),
    }
}

fn user_ty() -> Ty {
    Ty::nominal(NominalTy::bare(type_def(0)))
}

fn project_ty() -> Ty {
    Ty::nominal(NominalTy::bare(type_def(1)))
}

fn closure_ty(index: usize) -> Ty {
    Ty::closure(ClosureTyId::new(ExprId(index)))
}

fn vec_ty(inner: InferTy) -> InferTy {
    InferTy::Nominal(InferNominalTy {
        def: type_def(10),
        args: vec![InferGenericArg::Type(Box::new(inner))],
    })
}

fn concrete_vec_ty(inner: Ty) -> Ty {
    Ty::nominal(NominalTy {
        def: type_def(10),
        args: vec![GenericArg::Type(Box::new(inner))],
    })
}

fn opaque_bound(trait_index: usize, arg: InferTy) -> InferOpaqueTraitBound {
    InferOpaqueTraitBound {
        trait_ref: trait_ref(trait_index),
        args: vec![InferGenericArg::Type(Box::new(arg))],
    }
}

fn opaque_ty(bounds: Vec<InferOpaqueTraitBound>) -> InferTy {
    InferTy::Opaque {
        bounds: bounds.into_iter().collect(),
    }
}

#[test]
fn finalizes_unsolved_variables_to_stable_fallbacks() {
    let mut table = InferenceTable::new();

    let ty_var = table.new_type_var();
    let int_var = table.new_integer_var();
    let float_var = table.new_float_var();

    assert_eq!(table.finalize(&ty_var), Ty::Unknown);
    assert_eq!(
        table.finalize(&int_var),
        Ty::Primitive(PrimitiveTy::SignedInt(SignedIntTy::I32))
    );
    assert_eq!(
        table.finalize(&float_var),
        Ty::Primitive(PrimitiveTy::Float(FloatTy::F64))
    );
}

#[test]
fn conflicting_variables_finalize_to_unknown() {
    let mut table = InferenceTable::new();
    let var = table.new_type_var();

    assert!(table.unify(&var, &InferTy::Primitive(PrimitiveTy::Bool)));
    assert!(table.unify(&var, &InferTy::Primitive(PrimitiveTy::Char)));

    assert_eq!(table.finalize(&var), Ty::Unknown);
}

#[test]
fn unknown_does_not_solve_variables() {
    let mut table = InferenceTable::new();
    let var = table.new_type_var();

    assert!(!table.unify(&var, &InferTy::Unknown));

    assert_eq!(table.finalize(&var), Ty::Unknown);
}

#[test]
fn numeric_variables_accept_matching_primitive_evidence() {
    let mut table = InferenceTable::new();
    let int_var = table.new_integer_var();
    let float_var = table.new_float_var();

    assert!(table.unify(
        &int_var,
        &InferTy::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
    ));
    assert!(table.unify(
        &float_var,
        &InferTy::Primitive(PrimitiveTy::Float(FloatTy::F32))
    ));

    assert_eq!(
        table.finalize(&int_var),
        Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
    );
    assert_eq!(
        table.finalize(&float_var),
        Ty::Primitive(PrimitiveTy::Float(FloatTy::F32))
    );
}

#[test]
fn numeric_variables_follow_already_solved_type_variables() {
    let mut table = InferenceTable::new();
    let type_var = table.new_type_var();
    let int_var = table.new_integer_var();

    assert!(table.unify(
        &type_var,
        &InferTy::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
    ));
    assert!(table.unify(&int_var, &type_var));

    assert_eq!(
        table.finalize(&int_var),
        Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
    );
    assert_eq!(
        table.finalize(&type_var),
        Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
    );
}

#[test]
fn finalizes_solved_variables_inside_nominal_containers() {
    let mut table = InferenceTable::new();
    let element = table.new_type_var();

    assert!(table.unify(&element, &InferTy::from_ty(&user_ty())));

    assert_eq!(
        table.finalize(&vec_ty(element)),
        Ty::nominal(NominalTy {
            def: type_def(10),
            args: vec![GenericArg::Type(Box::new(user_ty()))],
        })
    );
}

#[test]
fn closure_types_round_trip_through_inference_family() {
    let table = InferenceTable::new();
    let ty = closure_ty(7);
    let infer_ty = InferTy::from_ty(&ty);

    assert_eq!(infer_ty, InferTy::Closure(ClosureTyId::new(ExprId(7))));
    assert_eq!(table.finalize(&infer_ty), ty);
}

#[test]
fn resolves_root_variables_without_replacing_nested_vars() {
    let mut table = InferenceTable::new();
    let element = table.new_type_var();
    let nested = table.new_type_var();

    assert!(table.unify(&element, &vec_ty(nested.clone())));

    assert_eq!(table.resolve_root_var(&element), vec_ty(nested.clone()));
    assert!(table.unify(&nested, &InferTy::from_ty(&user_ty())));
    assert_eq!(table.resolve_root_var(&element), vec_ty(nested));
    assert_eq!(table.finalize(&element), concrete_vec_ty(user_ty()));
}

#[test]
fn existing_var_links_do_not_create_reverse_cycles() {
    let mut table = InferenceTable::new();
    let left = table.new_type_var();
    let right = table.new_type_var();
    let joined = table.new_type_var();

    assert!(table.unify(&right, &left));
    assert!(!table.unify(&left, &right));
    assert!(table.unify(&joined, &left));
    assert!(!table.unify(&joined, &right));

    assert_eq!(table.resolve_root_var(&right), left);
    assert_eq!(table.resolve_root_var(&joined), left);
}

#[test]
fn indirect_var_links_do_not_create_reverse_cycles() {
    let mut table = InferenceTable::new();
    let first = table.new_type_var();
    let second = table.new_type_var();
    let third = table.new_type_var();
    let fourth = table.new_type_var();

    assert!(table.unify(&first, &second));
    assert!(table.unify(&second, &third));
    assert!(!table.unify(&third, &first));

    assert_eq!(table.resolve_root_var(&first), third);
    assert_eq!(table.resolve_root_var(&second), third);

    assert!(table.unify(&third, &fourth));
    assert!(!table.unify(&fourth, &first));
    assert_eq!(table.resolve_root_var(&first), fourth);
    assert_eq!(table.resolve_root_var(&second), fourth);
    assert_eq!(table.resolve_root_var(&third), fourth);
}

#[test]
fn canonicalizes_variable_aliases_inside_type_shapes() {
    let mut table = InferenceTable::new();
    let element = table.new_type_var();
    let alias = table.new_type_var();

    assert!(table.unify(&element, &alias));

    assert_eq!(table.canonicalize(&element), alias);
    assert_eq!(table.canonicalize(&vec_ty(element)), vec_ty(alias));
}

#[test]
fn canonicalize_expands_solved_slots_inside_type_shapes() {
    let mut table = InferenceTable::new();
    let element = table.new_type_var();

    assert!(table.unify(&element, &InferTy::from_ty(&user_ty())));

    assert_eq!(table.canonicalize(&element), InferTy::from_ty(&user_ty()));
    assert_eq!(
        table.canonicalize(&vec_ty(element.clone())),
        vec_ty(InferTy::from_ty(&user_ty()))
    );
    assert_eq!(table.finalize(&element), user_ty());
}

#[test]
fn later_evidence_refines_unknown_children_inside_solved_slots() {
    let mut table = InferenceTable::new();
    let values = table.new_type_var();
    let element = table.new_type_var();

    assert!(table.unify(&values, &vec_ty(InferTy::Unknown)));
    assert!(table.unify(&values, &vec_ty(element.clone())));
    assert!(table.unify(&element, &InferTy::from_ty(&user_ty())));

    assert_eq!(table.finalize(&values), concrete_vec_ty(user_ty()));
}

#[test]
fn single_opaque_bound_infers_through_matching_trait_args() {
    let mut table = InferenceTable::new();
    let element = table.new_type_var();

    assert!(table.unify(
        &opaque_ty(vec![opaque_bound(0, element.clone())]),
        &opaque_ty(vec![opaque_bound(0, InferTy::from_ty(&user_ty()))])
    ));

    assert_eq!(table.finalize(&element), user_ty());
}

#[test]
fn broad_opaque_bounds_do_not_infer_from_partial_bound_overlap() {
    let mut table = InferenceTable::new();
    let opaque = table.new_type_var();
    let element = table.new_type_var();

    assert!(table.unify(
        &opaque,
        &opaque_ty(vec![
            opaque_bound(0, element.clone()),
            opaque_bound(1, InferTy::from_ty(&project_ty())),
        ])
    ));
    assert!(!table.unify(
        &opaque,
        &opaque_ty(vec![opaque_bound(0, InferTy::from_ty(&user_ty()))])
    ));

    assert_eq!(table.finalize(&element), Ty::Unknown);
    assert!(matches!(table.finalize(&opaque), Ty::Opaque { .. }));
}

#[test]
fn unifies_same_definition_nominal_generic_arguments() {
    let mut table = InferenceTable::new();
    let element = table.new_type_var();

    assert!(table.unify(
        &vec_ty(element.clone()),
        &vec_ty(InferTy::from_ty(&user_ty()))
    ));

    assert_eq!(
        table.finalize(&element),
        Ty::nominal(NominalTy::bare(type_def(0)))
    );
}

#[test]
fn instantiates_unknowns_nested_inside_known_shapes() {
    let mut table = InferenceTable::new();
    let inferred = {
        let mut builder = UnknownTypeInstantiationBuilder::new(&mut table);
        let inferred = builder.ty_from_ty(&concrete_vec_ty(Ty::Unknown));
        assert!(builder.used_type_vars());
        inferred
    };

    assert!(table.unify(&inferred, &InferTy::from_ty(&concrete_vec_ty(user_ty()))));

    assert_eq!(table.finalize(&inferred), concrete_vec_ty(user_ty()));
}

#[test]
fn leaves_root_unknown_uninstantiated() {
    let mut table = InferenceTable::new();
    let mut builder = UnknownTypeInstantiationBuilder::new(&mut table);

    assert_eq!(builder.ty_from_ty(&Ty::Unknown), InferTy::Unknown);
    assert!(!builder.used_type_vars());
}

#[test]
fn explicit_type_arg_builder_instantiates_root_infer() {
    let mut table = InferenceTable::new();
    let inferred = {
        let mut builder = ExplicitTypeArgInstantiationBuilder::new(&mut table);
        let inferred = builder.ty_from_arg(&TypeRef::Infer, &Ty::Unknown);
        assert!(builder.used_type_vars());
        inferred
    };

    assert!(table.unify(&inferred, &InferTy::from_ty(&user_ty())));

    assert_eq!(table.finalize(&inferred), user_ty());
}

#[test]
fn explicit_type_arg_builder_instantiates_nested_infer() {
    let mut table = InferenceTable::new();
    let inferred = {
        let mut builder = ExplicitTypeArgInstantiationBuilder::new(&mut table);
        let inferred = builder.ty_from_arg(
            &TypeRef::Tuple(vec![TypeRef::Infer]),
            &Ty::Tuple(vec![Ty::Unknown]),
        );
        assert!(builder.used_type_vars());
        inferred
    };

    assert!(table.unify(&inferred, &InferTy::from_ty(&Ty::Tuple(vec![user_ty()]))));

    assert_eq!(table.finalize(&inferred), Ty::Tuple(vec![user_ty()]));
}

#[test]
fn explicit_type_arg_builder_preserves_concrete_args() {
    let mut table = InferenceTable::new();
    let mut builder = ExplicitTypeArgInstantiationBuilder::new(&mut table);
    let inferred = builder.ty_from_arg(
        &TypeRef::Tuple(vec![TypeRef::Unit]),
        &Ty::Tuple(vec![Ty::Unit]),
    );

    assert!(!builder.used_type_vars());
    assert_eq!(table.finalize(&inferred), Ty::Tuple(vec![Ty::Unit]));
}

#[test]
fn conflicting_nominal_variables_finalize_to_unknown() {
    let mut table = InferenceTable::new();
    let var = table.new_type_var();

    assert!(table.unify(&var, &InferTy::from_ty(&user_ty())));
    assert!(table.unify(&var, &InferTy::from_ty(&project_ty())));

    assert_eq!(table.finalize(&var), Ty::Unknown);
}
