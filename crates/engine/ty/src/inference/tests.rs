use rg_ir_model::{
    CrateRef, DefMapRef, ExprId, FunctionId, FunctionRef, GenericDefRef, OpaqueTyId, OpaqueTyRef,
    PackageSlot, StructId, TraitDefRef, TraitId, TypeDefId, TypeDefRef,
    items::{FloatTy, SignedIntTy, UnsignedIntTy},
};

use super::{InferenceTable, UnknownTypeInstantiationBuilder};
use crate::{
    AdtTy, AliasTy, Clause, ClosureTyId, GenericArg, GenericArgs, OpaqueTy, PrimitiveTy,
    TraitApplication, Ty,
};

fn def_map_ref() -> DefMapRef {
    DefMapRef::Crate(CrateRef {
        package: PackageSlot(0),
        crate_id: rg_ir_model::CrateId(0),
    })
}

fn type_def(index: usize) -> TypeDefRef {
    TypeDefRef {
        origin: def_map_ref(),
        id: TypeDefId::Struct(StructId(index)),
    }
}

fn user_ty() -> Ty {
    Ty::adt(AdtTy::bare(type_def(0)))
}

fn project_ty() -> Ty {
    Ty::adt(AdtTy::bare(type_def(1)))
}

fn closure_ty(index: usize) -> Ty {
    Ty::closure(ClosureTyId::new(ExprId(index)))
}

fn fn_def_ty(index: usize) -> Ty {
    Ty::fn_def(FunctionRef {
        origin: def_map_ref(),
        id: FunctionId(index),
    })
}

fn vec_ty(inner: Ty) -> Ty {
    Ty::Adt(AdtTy {
        def: type_def(10),
        args: vec![GenericArg::Type(Box::new(inner))].into(),
    })
}

fn concrete_vec_ty(inner: Ty) -> Ty {
    Ty::adt(AdtTy {
        def: type_def(10),
        args: vec![GenericArg::Type(Box::new(inner))].into(),
    })
}

fn opaque_ty(owner_index: usize, occurrence: usize, arg: Ty) -> Ty {
    Ty::Alias(AliasTy::Opaque(OpaqueTy {
        opaque: OpaqueTyRef {
            owner: GenericDefRef::Function(FunctionRef {
                origin: def_map_ref(),
                id: FunctionId(owner_index),
            }),
            id: OpaqueTyId(occurrence),
        },
        args: vec![GenericArg::Type(Box::new(arg))].into(),
    }))
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

    assert!(table.unify(&var, &Ty::Primitive(PrimitiveTy::Bool)));
    assert!(table.unify(&var, &Ty::Primitive(PrimitiveTy::Char)));

    assert_eq!(table.finalize(&var), Ty::Unknown);
}

#[test]
fn unknown_does_not_solve_variables() {
    let mut table = InferenceTable::new();
    let var = table.new_type_var();

    assert!(!table.unify(&var, &Ty::Unknown));

    assert_eq!(table.finalize(&var), Ty::Unknown);
}

#[test]
fn numeric_variables_accept_matching_primitive_evidence() {
    let mut table = InferenceTable::new();
    let int_var = table.new_integer_var();
    let float_var = table.new_float_var();

    assert!(table.unify(
        &int_var,
        &Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
    ));
    assert!(table.unify(&float_var, &Ty::Primitive(PrimitiveTy::Float(FloatTy::F32))));

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
        &Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
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

    assert!(table.unify(&element, &user_ty()));

    assert_eq!(
        table.finalize(&vec_ty(element)),
        Ty::adt(AdtTy {
            def: type_def(10),
            args: vec![GenericArg::Type(Box::new(user_ty()))].into(),
        })
    );
}

#[test]
fn wincode_rejects_transient_inference_vars() {
    let mut table = InferenceTable::new();
    let ty = vec_ty(table.new_type_var());

    assert!(wincode::serialize(&ty).is_err());
}

#[test]
fn closure_types_round_trip_through_inference_traversal() {
    let table = InferenceTable::new();
    let ty = closure_ty(7);
    let infer_ty = ty.clone();

    assert_eq!(infer_ty, Ty::Closure(ClosureTyId::new(ExprId(7))));
    assert_eq!(table.finalize(&infer_ty), ty);
}

#[test]
fn fn_def_types_round_trip_through_inference_traversal() {
    let table = InferenceTable::new();
    let function = FunctionRef {
        origin: def_map_ref(),
        id: FunctionId(7),
    };
    let ty = fn_def_ty(7);
    let infer_ty = ty.clone();

    assert_eq!(infer_ty, Ty::fn_def(function));
    assert_eq!(table.finalize(&infer_ty), ty);
}

#[test]
fn resolves_root_variables_without_replacing_nested_vars() {
    let mut table = InferenceTable::new();
    let element = table.new_type_var();
    let nested = table.new_type_var();

    assert!(table.unify(&element, &vec_ty(nested.clone())));

    assert_eq!(table.resolve_root_var(&element), vec_ty(nested.clone()));
    assert!(table.unify(&nested, &user_ty()));
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

    assert!(table.unify(&element, &user_ty()));

    assert_eq!(table.canonicalize(&element), user_ty());
    assert_eq!(
        table.canonicalize(&vec_ty(element.clone())),
        vec_ty(user_ty())
    );
    assert_eq!(table.finalize(&element), user_ty());
}

#[test]
fn canonicalizes_solved_slots_inside_trait_clauses() {
    let mut table = InferenceTable::new();
    let subject = table.new_type_var();
    assert!(table.unify(&subject, &user_ty()));
    let trait_ref = TraitDefRef {
        origin: def_map_ref(),
        id: TraitId(0),
    };

    let clause = Clause::Implemented(TraitApplication {
        def: trait_ref,
        args: vec![GenericArg::Type(Box::new(subject))].into(),
    });

    assert_eq!(
        table.canonicalize_clause(&clause),
        Clause::Implemented(TraitApplication {
            def: trait_ref,
            args: vec![GenericArg::Type(Box::new(user_ty()))].into(),
        })
    );
}

#[test]
fn generic_arg_equivalence_renames_inference_ids_bijectively() {
    let mut table = InferenceTable::new();
    let lhs_var = table.new_type_var();
    let rhs_var = table.new_type_var();
    let distinct_rhs_var = table.new_type_var();

    let lhs: GenericArgs = vec![
        GenericArg::Type(Box::new(vec_ty(lhs_var.clone()))),
        GenericArg::Type(Box::new(lhs_var)),
    ]
    .into();
    let equivalent_rhs: GenericArgs = vec![
        GenericArg::Type(Box::new(vec_ty(rhs_var.clone()))),
        GenericArg::Type(Box::new(rhs_var.clone())),
    ]
    .into();
    let distinct_rhs: GenericArgs = vec![
        GenericArg::Type(Box::new(vec_ty(rhs_var))),
        GenericArg::Type(Box::new(distinct_rhs_var)),
    ]
    .into();

    assert!(lhs.equivalent_modulo_inference_ids(&equivalent_rhs));
    assert!(!lhs.equivalent_modulo_inference_ids(&distinct_rhs));
}

#[test]
fn later_evidence_refines_unknown_children_inside_solved_slots() {
    let mut table = InferenceTable::new();
    let values = table.new_type_var();
    let element = table.new_type_var();

    assert!(table.unify(&values, &vec_ty(Ty::Unknown)));
    assert!(table.unify(&values, &vec_ty(element.clone())));
    assert!(table.unify(&element, &user_ty()));

    assert_eq!(table.finalize(&values), concrete_vec_ty(user_ty()));
}

#[test]
fn same_opaque_occurrence_infers_through_generic_args() {
    let mut table = InferenceTable::new();
    let element = table.new_type_var();

    assert!(table.unify(
        &opaque_ty(0, 0, element.clone()),
        &opaque_ty(0, 0, user_ty())
    ));

    assert_eq!(table.finalize(&element), user_ty());
}

#[test]
fn distinct_opaque_occurrences_do_not_unify_even_under_one_owner() {
    let mut table = InferenceTable::new();
    let element = table.new_type_var();

    assert!(!table.unify(
        &opaque_ty(0, 0, element.clone()),
        &opaque_ty(0, 1, user_ty())
    ));

    assert_eq!(table.finalize(&element), Ty::Unknown);
}

#[test]
fn unifies_same_definition_nominal_generic_arguments() {
    let mut table = InferenceTable::new();
    let element = table.new_type_var();

    assert!(table.unify(&vec_ty(element.clone()), &vec_ty(user_ty())));

    assert_eq!(table.finalize(&element), Ty::adt(AdtTy::bare(type_def(0))));
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

    assert!(table.unify(&inferred, &concrete_vec_ty(user_ty())));

    assert_eq!(table.finalize(&inferred), concrete_vec_ty(user_ty()));
}

#[test]
fn leaves_root_unknown_uninstantiated() {
    let mut table = InferenceTable::new();
    let mut builder = UnknownTypeInstantiationBuilder::new(&mut table);

    assert_eq!(builder.ty_from_ty(&Ty::Unknown), Ty::Unknown);
    assert!(!builder.used_type_vars());
}

#[test]
fn conflicting_nominal_variables_finalize_to_unknown() {
    let mut table = InferenceTable::new();
    let var = table.new_type_var();

    assert!(table.unify(&var, &user_ty()));
    assert!(table.unify(&var, &project_ty()));

    assert_eq!(table.finalize(&var), Ty::Unknown);
}
