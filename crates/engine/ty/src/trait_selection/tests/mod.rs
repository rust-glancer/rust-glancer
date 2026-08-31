mod utils;

use std::fmt::Write as _;

use expect_test::expect;
use rg_ir_model::{
    CrateId, CrateRef, DefMapRef, PackageSlot, StructId, TypeAliasId, TypeAliasRef, TypeDefRef,
};
use rg_semantic_ir::CrateItemQuery;
use rg_std::CancellationToken;

use self::utils::*;
use super::chalk::{ChalkInferenceCache, ChalkOutcome, ChalkTraitSolver};
use super::projection::NORMALIZATION_DEPTH_LIMIT;
use super::{TraitCandidate, TraitGoal, TraitSelectionSession};
use crate::inference::InferenceTable;
use crate::{
    AdtTy, AliasTy, Clause, GenericArg, ImplMatcher, ItemPathQuery, ProjectionTy, Ty, TyContext,
};

#[test]
fn named_trait_discovery_ignores_unrelated_blanket_impls() {
    const UNRELATED_TRAITS: usize = 24;

    let mut source = String::from("traits\n");
    for index in 0..UNRELATED_TRAITS {
        writeln!(source, "  trait#{index} Noise{index}")
            .expect("writing to a string should not fail");
    }
    writeln!(source, "  trait#{UNRELATED_TRAITS} Target")
        .expect("writing to a string should not fail");
    source.push_str("structs\n  struct#0 User\nimpls\n");
    for index in 0..UNRELATED_TRAITS {
        writeln!(
            source,
            "  impl#{index} impl<T> Noise{index} for T [resolved self: empty]"
        )
        .expect("writing to a string should not fail");
    }
    writeln!(
        source,
        "  impl#{UNRELATED_TRAITS} impl Target for User\nfunctions\n  fn#0 Target::target -> User"
    )
    .expect("writing to a string should not fail");

    let fixture = TraitSelectionFixture::new(&source);
    let lookup = fixture.lookup_query();
    let relevant_traits = lookup.traits_with_function_name("target");
    assert_eq!(
        relevant_traits.as_slice(),
        &[fixture
            .trait_ref_by_name("Target")
            .expect("fixture should contain Target")]
    );

    // Selecting the trait by its declaration surface and probing its one impl fit this small
    // allowance. A receiver-wide scan would spend it on the preceding blanket impls before
    // reaching Target.
    let session = TraitSelectionSession::new(fixture.target).with_work_limit(4);
    let context = TyContext::new(&fixture, &fixture, lookup, session);
    let matcher = ImplMatcher::new(context);
    let receiver_ty = Ty::adt(AdtTy {
        def: fixture
            .type_ref_by_name("User")
            .expect("fixture should contain User"),
        args: Vec::new().into(),
    });
    let matches = matcher
        .matches_for_receiver_with_traits(&receiver_ty, relevant_traits, &InferenceTable::new())
        .expect("bounded named trait lookup should succeed");

    assert_eq!(matches.traits().len(), 1);
    assert_eq!(
        matches.traits()[0].trait_impl.trait_ref,
        fixture
            .trait_ref_by_name("Target")
            .expect("fixture should contain Target")
    );
}

#[test]
fn broad_trait_candidates_are_charged_once_per_inference_scope() {
    let fixture = TraitSelectionFixture::new(
        r#"
            traits
              trait#0 Marker
            structs
              struct#0 First
              struct#1 Second
            impls
              impl#0 impl Marker for First
              impl#1 impl Marker for Second
        "#,
    );
    let lookup = fixture.lookup_query();
    let trait_ref = fixture
        .trait_ref_by_name("Marker")
        .expect("fixture should contain Marker");
    let unresolved_projection = Ty::Alias(AliasTy::Projection(ProjectionTy {
        associated_ty: TypeAliasRef {
            origin: origin(),
            id: TypeAliasId(0),
        },
        args: Vec::new().into(),
    }));
    let session = TraitSelectionSession::new(fixture.target).with_work_limit(2);

    let first = session
        .trait_impl_candidates_for_ty(&lookup, trait_ref, &unresolved_projection)
        .expect("the first broad lookup should fit the exact allowance");
    let repeated = session
        .trait_impl_candidates_for_ty(&lookup, trait_ref, &unresolved_projection)
        .expect("reusing the same broad lookup should consume no more work");

    assert_eq!(first.len(), 2);
    assert_eq!(repeated, first);
}

#[test]
fn projection_cycle_identity_ignores_fresh_inference_slots() {
    let mut table = InferenceTable::new();
    let first_slot = table.new_type_var();
    let fresh_slot = table.new_type_var();
    let associated_ty = TypeAliasRef {
        origin: origin(),
        id: TypeAliasId(0),
    };
    let first = ProjectionTy {
        associated_ty,
        args: vec![GenericArg::Type(Box::new(first_slot))].into(),
    };
    let repeated = ProjectionTy {
        associated_ty,
        args: vec![GenericArg::Type(Box::new(fresh_slot))].into(),
    };
    let unrelated = ProjectionTy {
        associated_ty: TypeAliasRef {
            origin: origin(),
            id: TypeAliasId(1),
        },
        args: repeated.args.clone(),
    };

    assert_ne!(first, repeated);
    assert!(first.equivalent_modulo_inference_ids(&repeated));
    assert!(!first.equivalent_modulo_inference_ids(&unrelated));
}

#[test]
fn possible_impl_origins_include_nested_known_type_owners() {
    let outer_crate = CrateRef {
        package: PackageSlot(1),
        crate_id: CrateId(0),
    };
    let nested_sibling_crate = CrateRef {
        package: PackageSlot(1),
        crate_id: CrateId(1),
    };
    let positional_crate = CrateRef {
        package: PackageSlot(2),
        crate_id: CrateId(0),
    };
    let nested_ty = Ty::adt(AdtTy::bare(TypeDefRef::new_struct(
        DefMapRef::Crate(nested_sibling_crate),
        StructId(0),
    )));
    let mut table = InferenceTable::new();
    let nested_slot = table.new_type_var();
    assert!(table.unify(&nested_slot, &nested_ty));

    let outer_ty = Ty::adt(AdtTy {
        def: TypeDefRef::new_struct(DefMapRef::Crate(outer_crate), StructId(0)),
        args: vec![GenericArg::Type(Box::new(nested_slot))].into(),
    });
    let positional_ty = Ty::Tuple(vec![Ty::adt(AdtTy::bare(TypeDefRef::new_struct(
        DefMapRef::Crate(positional_crate),
        StructId(0),
    )))]);
    let mut goal = TraitGoal::new(
        outer_ty,
        trait_ref(0),
        vec![GenericArg::Type(Box::new(positional_ty))],
    );
    // Output constraints do not participate in orphan ownership.
    goal.associated_types.push(crate::AssocTypeBinding {
        associated_ty: TypeAliasRef {
            origin: origin(),
            id: TypeAliasId(0),
        },
        ty: Ty::Unknown,
    });

    let origins = goal
        .possible_impl_origins(&table)
        .expect("solved nested inference should leave a fully known application");
    let expected_origins = [
        target(),
        outer_crate,
        nested_sibling_crate,
        positional_crate,
    ];
    assert_eq!(origins.len(), expected_origins.len());
    for expected_origin in expected_origins {
        assert!(
            origins.contains(&expected_origin),
            "expected owner collection to include {expected_origin:?}"
        );
    }
}

#[test]
fn possible_impl_origins_decline_unresolved_applications() {
    let outer_ty = |argument| {
        Ty::adt(AdtTy {
            def: type_def(0),
            args: vec![GenericArg::Type(Box::new(argument))].into(),
        })
    };
    let mut table = InferenceTable::new();
    let unresolved_slot = table.new_type_var();
    let unresolved_projection = Ty::Alias(AliasTy::Projection(ProjectionTy {
        associated_ty: TypeAliasRef {
            origin: origin(),
            id: TypeAliasId(0),
        },
        args: Vec::new().into(),
    }));

    for (argument, error) in [
        (Ty::Unknown, "semantic unknown"),
        (unresolved_slot, "inference variable"),
        (unresolved_projection, "associated projection"),
    ] {
        let goal = TraitGoal::new(outer_ty(argument), trait_ref(0), Vec::new());
        assert!(
            goal.possible_impl_origins(&table).is_none(),
            "{error} should disable coherence filtering"
        );
    }
}

#[test]
fn coherence_filter_retains_unknown_impl_from_goal_type_owner() {
    let dependency = CrateRef {
        package: PackageSlot(1),
        crate_id: CrateId(0),
    };
    let fixture = TraitSelectionFixture::new(
        r#"
            traits
              trait#0 Iterator
        "#,
    )
    .with_unknown_self_impl_dependency(dependency, "Iterator");
    let goal = TraitGoal::new(
        Ty::adt(AdtTy::bare(TypeDefRef::new_struct(
            DefMapRef::Crate(dependency),
            StructId(0),
        ))),
        fixture
            .trait_ref_by_name("Iterator")
            .expect("fixture should contain Iterator"),
        Vec::new(),
    );
    let lookup = fixture.lookup_query();
    let candidates = TraitCandidate::plausible_impls(
        &lookup,
        &TraitSelectionSession::new(fixture.target),
        &goal,
        &InferenceTable::new(),
    )
    .expect("candidate lookup should not exhaust work");

    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates
            .as_one()
            .map(|candidate| candidate.impl_ref.origin),
        Some(DefMapRef::Crate(dependency))
    );
}

#[test]
fn concrete_projection_skips_unrelated_unknown_impl_origin() {
    let profile = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "ty.trait_selection.chalk",
    );
    let fixture = TraitSelectionFixture::new(
        r#"
            traits
              trait#0 Iterator
            structs
              struct#0 Wrapper<T>
              struct#1 User
            type aliases
              type#0 trait#0::Item
        "#,
    )
    .with_unknown_self_impl_dependency(
        CrateRef {
            package: PackageSlot(1),
            crate_id: CrateId(0),
        },
        "Iterator",
    );
    let parsed = TraitSelectionQueryParser::new(&fixture)
        .parse_assoc_goal("<Wrapper<User> as Iterator>::Item");

    let result = query(&fixture)
        .normalize_assoc_type(&parsed.goal, &parsed.assoc_name, &parsed.table)
        .expect("concrete negative projection should complete natively");
    assert!(result.is_none());

    let profile = profile.finish();
    profile.assert_counter(crate::profile::metric::NATIVE_CANDIDATE_COHERENCE_SKIPS, 1);
    assert_eq!(
        profile
            .inner()
            .counter(crate::profile::metric::PROGRAM_BUILDS.path()),
        None,
        "coherence-excluded candidates should not construct a Chalk program",
    );
}

#[test]
fn body_work_exhaustion_keeps_candidate_search_incomplete() {
    let profile = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "ty.trait_selection.chalk",
    );
    let fixture = TraitSelectionFixture::new(
        r#"
            traits
              trait#0 Marker
            structs
              struct#0 User
            impls
              impl#0 impl Marker for User
        "#,
    );
    let parsed = TraitSelectionQueryParser::new(&fixture).parse_goal("User: Marker");
    let query = query_with_session(
        &fixture,
        TraitSelectionSession::new(fixture.target).with_work_limit(0),
    );

    let (selection, complete) = query
        .probe_with_completeness(&parsed.goal, &parsed.table)
        .expect("bounded candidate query should not fail");

    assert!(selection.is_empty());
    assert!(
        !complete,
        "work exhaustion must not prove candidate absence"
    );
    profile.finish().assert_keyed_counter(
        crate::profile::metric::WORK_LIMIT_EXHAUSTIONS,
        "body_work.candidate_probe",
        1,
    );
}

#[test]
fn program_work_exhaustion_does_not_publish_a_partial_extension() {
    let fixture = TraitSelectionFixture::new(
        r#"
            traits
              trait#0 Marker
            structs
              struct#0 User
            impls
              impl#0 impl Marker for User
        "#,
    );
    let parsed = TraitSelectionQueryParser::new(&fixture).parse_goal("User: Marker");
    let clauses = [Clause::Implemented(parsed.goal.application)];
    let item_paths = ItemPathQuery::new(&fixture, &fixture);
    let crate_items = CrateItemQuery::new(&fixture, &fixture, fixture.target);
    let solver = ChalkTraitSolver::new();
    let lookup_query = fixture.lookup_query();

    let limited = TraitSelectionSession::new(fixture.target).with_work_limit(1);
    let outcome = solver
        .prove_clauses(
            &item_paths,
            &crate_items,
            &lookup_query,
            &limited,
            &ChalkInferenceCache::new(),
            &clauses,
            &parsed.table,
        )
        .expect("bounded Chalk query should not fail");
    assert!(matches!(outcome, ChalkOutcome::Exhausted));

    // Discovery builds a temporary scope. A later unbounded query must be able to materialize the
    // same roots from scratch rather than observe a half-published solver database.
    let outcome = solver
        .prove_clauses(
            &item_paths,
            &crate_items,
            &lookup_query,
            &TraitSelectionSession::new(fixture.target),
            &ChalkInferenceCache::new(),
            &clauses,
            &parsed.table,
        )
        .expect("retry after bounded Chalk discovery should not fail");
    assert!(matches!(outcome, ChalkOutcome::Proven(_)));
}

#[test]
fn cancelled_trait_program_stops_without_publishing_a_partial_extension() {
    let fixture = TraitSelectionFixture::new(
        r#"
            traits
              trait#0 Marker
            structs
              struct#0 User
            impls
              impl#0 impl Marker for User
        "#,
    );
    let parsed = TraitSelectionQueryParser::new(&fixture).parse_goal("User: Marker");
    let clauses = [Clause::Implemented(parsed.goal.application)];
    let item_paths = ItemPathQuery::new(&fixture, &fixture);
    let crate_items = CrateItemQuery::new(&fixture, &fixture, fixture.target);
    let solver = ChalkTraitSolver::new();
    let lookup_query = fixture.lookup_query();
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let cancelled = TraitSelectionSession::new(fixture.target).with_cancellation(cancellation);
    let outcome = solver
        .prove_clauses(
            &item_paths,
            &crate_items,
            &lookup_query,
            &cancelled,
            &ChalkInferenceCache::new(),
            &clauses,
            &parsed.table,
        )
        .expect("cancelled Chalk query should fail soft");
    assert!(matches!(outcome, ChalkOutcome::Exhausted));

    let outcome = solver
        .prove_clauses(
            &item_paths,
            &crate_items,
            &lookup_query,
            &TraitSelectionSession::new(fixture.target),
            &ChalkInferenceCache::new(),
            &clauses,
            &parsed.table,
        )
        .expect("retry after cancelled Chalk discovery should not fail");
    assert!(matches!(outcome, ChalkOutcome::Proven(_)));
}

#[test]
fn speculative_recursive_blanket_goal_stays_pending() {
    let fixture = TraitSelectionFixture::new(
        r#"
            traits
              trait#0 Marker
            structs
              struct#0 Box<T>
              struct#1 User
            impls
              impl#0 impl Marker for User
              impl#1 impl<T: Marker> Marker for Box<T>
        "#,
    );
    let parsed = TraitSelectionQueryParser::new(&fixture).parse_goal("Box<?item>: Marker");
    let clauses = [Clause::Implemented(parsed.goal.application)];
    let item_paths = ItemPathQuery::new(&fixture, &fixture);
    let crate_items = CrateItemQuery::new(&fixture, &fixture, fixture.target);
    let outcome = ChalkTraitSolver::new()
        .prove_clauses(
            &item_paths,
            &crate_items,
            &fixture.lookup_query(),
            &TraitSelectionSession::new(fixture.target),
            &ChalkInferenceCache::new(),
            &clauses,
            &parsed.table,
        )
        .expect("recursive speculative Chalk query should not fail");

    assert!(matches!(outcome, ChalkOutcome::Exhausted));
}

#[test]
fn speculative_cross_trait_recursive_goal_stays_pending() {
    let fixture = TraitSelectionFixture::new(
        r#"
            traits
              trait#0 Marker
              trait#1 Step
            structs
              struct#0 Box<T>
              struct#1 Wrap<T>
              struct#2 User
            impls
              impl#0 impl<T: Step> Marker for Box<T>
              impl#1 impl<T: Marker> Step for Wrap<T>
              impl#2 impl Marker for User
        "#,
    );
    let parsed = TraitSelectionQueryParser::new(&fixture).parse_goal("Box<?item>: Marker");
    let clauses = [Clause::Implemented(parsed.goal.application)];
    let item_paths = ItemPathQuery::new(&fixture, &fixture);
    let crate_items = CrateItemQuery::new(&fixture, &fixture, fixture.target);
    let profile = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "ty.trait_selection.chalk",
    );
    let outcome = ChalkTraitSolver::new()
        .prove_clauses(
            &item_paths,
            &crate_items,
            &fixture.lookup_query(),
            &TraitSelectionSession::new(fixture.target),
            &ChalkInferenceCache::new(),
            &clauses,
            &parsed.table,
        )
        .expect("cross-trait recursive Chalk query should not fail");

    assert!(matches!(outcome, ChalkOutcome::Exhausted));
    assert_eq!(
        profile
            .finish()
            .inner()
            .counter(crate::profile::metric::SOLVER_GOALS.path()),
        None,
        "the speculative cycle should be rejected before solver search",
    );
}

#[test]
fn recursive_projection_normalization_stops_at_its_depth_limit() {
    let chain_len = NORMALIZATION_DEPTH_LIMIT + 6;
    let mut source = String::from("traits\n  trait#0 Next\nstructs\n");
    for index in 0..chain_len {
        writeln!(source, "  struct#{index} S{index}").expect("string writes should not fail");
    }
    writeln!(source, "  struct#{chain_len} User\nimpls").expect("string writes should not fail");
    for index in 0..chain_len {
        writeln!(source, "  impl#{index} impl Next for S{index}")
            .expect("string writes should not fail");
    }
    writeln!(source, "type aliases\n  type#0 trait#0::Item")
        .expect("string writes should not fail");
    for index in 0..chain_len {
        if index + 1 == chain_len {
            writeln!(source, "  type#{} impl#{index}::Item = User", index + 1)
                .expect("string writes should not fail");
        } else {
            writeln!(
                source,
                "  type#{} impl#{index}::Item = <S{} as Next>::Item",
                index + 1,
                index + 1,
            )
            .expect("string writes should not fail");
        }
    }

    let fixture = TraitSelectionFixture::new(&source);
    let profile = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "ty.trait_selection.chalk",
    );
    let projection = query(&fixture)
        .normalize_assoc_type(
            &TraitSelectionQueryParser::new(&fixture)
                .parse_assoc_goal("<S0 as Next>::Item")
                .goal,
            "Item",
            &InferenceTable::new(),
        )
        .expect("bounded projection query should not fail")
        .expect("the first projection should have an exact native impl");

    assert!(matches!(projection.ty, Ty::Alias(AliasTy::Projection(_))));
    profile.finish().assert_keyed_counter(
        crate::profile::metric::WORK_LIMIT_EXHAUSTIONS,
        "normalization_depth",
        1,
    );
}

#[test]
fn probe_selects_direct_from_iterator_impl_and_solves_destination_arg() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 FromIterator<T>
            structs
              struct#0 Vec<T>
              struct#1 User
            impls
              impl#0 impl<T> FromIterator<T> for Vec<T>
        "#,
        vec![TraitSelectionCase::probe(
            "select direct impl",
            "Vec<?item>: FromIterator<User>",
        )],
        expect![[r#"
            select direct impl
              query: selection
              goal: Vec<?item>: FromIterator<User>
              result: one
                impl: impl#0
                applicability: yes
                vars
                  ?item = User
        "#]],
    );
}

#[test]
fn normalize_assoc_type_projects_generic_impl_value() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Iterator
            structs
              struct#0 Iter<T>
              struct#1 User
            impls
              impl#0 impl<T> Iterator for Iter<T>
            type aliases
              type#0 trait#0::Item
              type#1 impl#0::Item = T
        "#,
        vec![TraitSelectionCase::normalize_assoc(
            "project generic impl Item",
            "<Iter<User> as Iterator>::Item",
        )],
        expect![[r#"
            project generic impl Item
              query: selection
              goal: <Iter<User> as Iterator>::Item
              result: projected
                infer: User
                final: User
                applicability: yes
        "#]],
    );
}

#[test]
fn probe_checks_goal_associated_type_equality_constraints() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Iterator
            structs
              struct#0 Iter<T>
              struct#1 User
              struct#2 Other
            impls
              impl#0 impl<T> Iterator for Iter<T>
            type aliases
              type#0 trait#0::Item
              type#1 impl#0::Item = T
        "#,
        vec![
            TraitSelectionCase::probe(
                "accept matching associated equality",
                "Iter<User>: Iterator<Item = User>",
            ),
            TraitSelectionCase::probe(
                "reject mismatched associated equality",
                "Iter<User>: Iterator<Item = Other>",
            ),
            TraitSelectionCase::probe(
                "solve receiver slot from associated equality",
                "Iter<?item>: Iterator<Item = User>",
            ),
        ],
        expect![[r#"
            accept matching associated equality
              query: selection
              goal: Iter<User>: Iterator<Item = User>
              result: one
                impl: impl#0
                applicability: yes

            reject mismatched associated equality
              query: selection
              goal: Iter<User>: Iterator<Item = Other>
              result: empty

            solve receiver slot from associated equality
              query: selection
              goal: Iter<?item>: Iterator<Item = User>
              result: one
                impl: impl#0
                applicability: yes
                vars
                  ?item = User
        "#]],
    );
}

#[test]
fn probe_checks_custom_trait_associated_type_equality_constraints() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Source
            structs
              struct#0 Bag<T>
              struct#1 User
            impls
              impl#0 impl<T> Source for Bag<T>
            type aliases
              type#0 trait#0::Output
              type#1 impl#0::Output = T
        "#,
        vec![TraitSelectionCase::probe(
            "solve custom associated equality",
            "Bag<?item>: Source<Output = User>",
        )],
        expect![[r#"
            solve custom associated equality
              query: selection
              goal: Bag<?item>: Source<Output = User>
              result: one
                impl: impl#0
                applicability: yes
                vars
                  ?item = User
        "#]],
    );
}

#[test]
fn native_proof_resolves_impl_predicate_associated_type_equality_constraints() {
    let profile = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "ty.trait_selection.chalk",
    );
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Iterator
              trait#1 AcceptsUserIterator
            structs
              struct#0 Iter<T>
              struct#1 User
              struct#2 Other
              struct#3 Adapter<I>
            impls
              impl#0 impl<T> Iterator for Iter<T>
              impl#1 impl<I: Iterator<Item = User>> AcceptsUserIterator for Adapter<I>
            type aliases
              type#0 trait#0::Item
              type#1 impl#0::Item = T
        "#,
        vec![
            TraitSelectionCase::probe(
                "prove matching impl predicate equality",
                "Adapter<Iter<User>>: AcceptsUserIterator",
            ),
            TraitSelectionCase::probe(
                "reject mismatched impl predicate equality",
                "Adapter<Iter<Other>>: AcceptsUserIterator",
            ),
        ],
        expect![[r#"
            prove matching impl predicate equality
              query: selection
              goal: Adapter<Iter<User>>: AcceptsUserIterator
              result: one
                impl: impl#1
                applicability: yes

            reject mismatched impl predicate equality
              query: selection
              goal: Adapter<Iter<Other>>: AcceptsUserIterator
              result: empty
        "#]],
    );
    let profile = profile.finish();
    profile.assert_counter(crate::profile::metric::NATIVE_ASSOC_PROJECTIONS, 2);
    assert_eq!(
        profile
            .inner()
            .counter(crate::profile::metric::PROGRAM_BUILDS.path()),
        None,
        "exact associated equalities should not construct a Chalk program",
    );
}

#[test]
fn probe_prefers_definite_impl_over_maybe_headers() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Iterator
            structs
              struct#0 Iter<T>
              struct#1 User
            impls
              impl#0 impl<T> Iterator for Iter<T>
              impl#1 impl Iterator for <unsupported:macro generated self type> [resolved self: empty]
        "#,
        vec![
            TraitSelectionCase::probe("default selection", "Iter<User>: Iterator"),
            TraitSelectionCase::candidate_probe("exploratory candidates", "Iter<User>: Iterator"),
        ],
        expect![[r#"
            default selection
              query: selection
              goal: Iter<User>: Iterator
              result: one
                impl: impl#0
                applicability: yes

            exploratory candidates
              query: candidate
              goal: Iter<User>: Iterator
              result: ambiguous
        "#]],
    );
}

#[test]
fn chalk_solver_normalizes_generic_associated_type_value() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Iterator
            structs
              struct#0 Iter<T>
              struct#1 User
            impls
              impl#0 impl<T> Iterator for Iter<T>
            type aliases
              type#0 trait#0::Item
              type#1 impl#0::Item = T
        "#,
        vec![TraitSelectionCase::chalk_normalize_assoc(
            "chalk project generic impl Item",
            "<Iter<User> as Iterator>::Item",
        )],
        expect![[r#"
            chalk project generic impl Item
              query: chalk
              goal: <Iter<User> as Iterator>::Item
              result: projected
                infer: User
                final: User
                applicability: yes
        "#]],
    );
}

#[test]
fn chalk_solver_normalizes_associated_type_to_existing_inference_var() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Iterator
            structs
              struct#0 Iter<T>
            impls
              impl#0 impl<T> Iterator for Iter<T>
            type aliases
              type#0 trait#0::Item
              type#1 impl#0::Item = T
        "#,
        vec![TraitSelectionCase::chalk_normalize_assoc(
            "chalk preserves projection variable",
            "<Iter<?item> as Iterator>::Item",
        )],
        expect![[r#"
            chalk preserves projection variable
              query: chalk
              goal: <Iter<?item> as Iterator>::Item
              result: projected
                infer: ?item
                final: _
                applicability: yes
                vars
                  ?item = _
        "#]],
    );
}

#[test]
fn chalk_solver_commits_projection_answer_evidence_to_inference_table() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Indexed<T>
            structs
              struct#0 Vec<T>
              struct#1 User
            impls
              impl#0 impl<T> Indexed<T> for Vec<T>
            type aliases
              type#0 trait#0::Item
              type#1 impl#0::Item = T
        "#,
        vec![TraitSelectionCase::chalk_normalize_assoc(
            "chalk solves projection variable",
            "<Vec<?item> as Indexed<User>>::Item",
        )],
        expect![[r#"
            chalk solves projection variable
              query: chalk
              goal: <Vec<?item> as Indexed<User>>::Item
              result: projected
                infer: User
                final: User
                applicability: yes
                vars
                  ?item = User
        "#]],
    );
}

#[test]
fn chalk_solver_normalizes_array_associated_type_value() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 ArrayProvider
            structs
              struct#0 Holder<T>
            impls
              impl#0 impl<T> ArrayProvider for Holder<T>
            type aliases
              type#0 trait#0::Item
              type#1 impl#0::Item = [T; 3]
        "#,
        vec![TraitSelectionCase::chalk_normalize_assoc(
            "chalk projects array impl Item",
            "<Holder<?item> as ArrayProvider>::Item",
        )],
        expect![[r#"
            chalk projects array impl Item
              query: chalk
              goal: <Holder<?item> as ArrayProvider>::Item
              result: projected
                infer: [?item; 3]
                final: [_; 3]
                applicability: yes
                vars
                  ?item = _
        "#]],
    );
}

#[test]
fn chalk_solver_raises_raw_pointer_and_function_pointer_values() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Shapes
            structs
              struct#0 Holder
              struct#1 User
            impls
              impl#0 impl Shapes for Holder
            type aliases
              type#0 trait#0::Pointer
              type#1 impl#0::Pointer = *const User
              type#2 trait#0::Callback
              type#3 impl#0::Callback = fn(User) -> User
        "#,
        vec![
            TraitSelectionCase::chalk_normalize_assoc(
                "chalk projects raw pointer",
                "<Holder as Shapes>::Pointer",
            ),
            TraitSelectionCase::chalk_normalize_assoc(
                "chalk projects function pointer",
                "<Holder as Shapes>::Callback",
            ),
        ],
        expect![[r#"
            chalk projects raw pointer
              query: chalk
              goal: <Holder as Shapes>::Pointer
              result: projected
                infer: *const User
                final: *const User
                applicability: yes

            chalk projects function pointer
              query: chalk
              goal: <Holder as Shapes>::Callback
              result: projected
                infer: fn(User) -> User
                final: fn(User) -> User
                applicability: yes
        "#]],
    );
}

#[test]
fn normalize_assoc_type_recurses_to_terminal_chalk_answer() {
    // The selected Chalk datum first produces `I::Item`. Recursive normalization feeds that
    // semantic projection back through the same adapter and reaches `User` without an impl-alias
    // side door.
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Iterator
            structs
              struct#0 Iter<T>
              struct#1 User
              struct#2 Skip<I>
            impls
              impl#0 impl<T> Iterator for Iter<T>
              impl#1 impl<I: Iterator> Iterator for Skip<I>
            type aliases
              type#0 trait#0::Item
              type#1 impl#0::Item = T
              type#2 impl#1::Item = <I as Iterator>::Item
        "#,
        vec![TraitSelectionCase::normalize_assoc(
            "project qualified impl Item",
            "<Skip<Iter<User>> as Iterator>::Item",
        )],
        expect![[r#"
            project qualified impl Item
              query: selection
              goal: <Skip<Iter<User>> as Iterator>::Item
              result: projected
                infer: User
                final: User
                applicability: yes
        "#]],
    );
}

#[test]
fn blanket_self_param_impl_and_source_opaque_bounds_are_proved() {
    // Pair blanket-impl selection with Chalk's terminal associated value for both a nominal
    // iterator and an opaque iterator. Opaque equality comes from its declared Chalk datum rather
    // than a source-side bounds lookup.
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Iterator
              trait#1 IntoIterator
            structs
              struct#0 Iter<T>
              struct#1 User
              struct#2 NotIter
            impls
              impl#0 impl<T> Iterator for Iter<T>
              impl#1 impl<I: Iterator> IntoIterator for I [resolved self: empty]
            type aliases
              type#0 trait#0::Item
              type#1 impl#0::Item = T
              type#2 trait#1::Item
              type#3 impl#1::Item = <I as Iterator>::Item
            functions
              fn#0 opaque_iter -> impl Iterator<Item = User>
        "#,
        vec![
            TraitSelectionCase::probe("prove blanket iterator impl", "Iter<User>: IntoIterator"),
            TraitSelectionCase::normalize_assoc(
                "project blanket IntoIterator Item",
                "<Iter<User> as IntoIterator>::Item",
            ),
            TraitSelectionCase::probe(
                "prove blanket iterator impl for opaque iterator",
                "opaque#0: IntoIterator",
            ),
            TraitSelectionCase::normalize_assoc(
                "project blanket opaque IntoIterator Item",
                "<opaque#0 as IntoIterator>::Item",
            ),
            TraitSelectionCase::probe(
                "reject unproved blanket iterator impl",
                "NotIter: IntoIterator",
            ),
        ],
        expect![[r#"
            prove blanket iterator impl
              query: selection
              goal: Iter<User>: IntoIterator
              result: one
                impl: impl#1
                applicability: yes

            project blanket IntoIterator Item
              query: selection
              goal: <Iter<User> as IntoIterator>::Item
              result: projected
                infer: User
                final: User
                applicability: yes

            prove blanket iterator impl for opaque iterator
              query: selection
              goal: impl Iterator<Item = User>: IntoIterator
              result: one
                impl: impl#1
                applicability: yes

            project blanket opaque IntoIterator Item
              query: selection
              goal: <impl Iterator<Item = User> as IntoIterator>::Item
              result: projected
                infer: User
                final: User
                applicability: yes

            reject unproved blanket iterator impl
              query: selection
              goal: NotIter: IntoIterator
              result: empty
        "#]],
    );
}

#[test]
fn blanket_impl_proves_nested_adapter_with_dependent_associated_bound() {
    // `Copied<I>` determines `T` through `I::Item = T` before it can prove `T: Copy`. Keep this
    // shaped like the standard iterator adapters: it exercises a blanket outer impl, nested
    // adapter predicates, and a generic that appears only in an associated equality.
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Copy
              trait#1 Iterator
              trait#2 IntoIterator
            structs
              struct#0 Iter<T>
              struct#1 Copied<I>
              struct#2 Enumerate<I>
              struct#3 User
              struct#4 Other
            impls
              impl#0 impl Copy for User
              impl#1 impl Copy for Other
              impl#2 impl<T> Iterator for Iter<T>
              impl#3 impl<I: Iterator<Item = T>, T: Copy> Iterator for Copied<I>
              impl#4 impl<I: Iterator> Iterator for Enumerate<I>
              impl#5 impl<I: Iterator> IntoIterator for I [resolved self: empty]
            type aliases
              type#0 trait#1::Item
              type#1 impl#2::Item = T
              type#2 impl#3::Item = T
              type#3 impl#4::Item = <I as Iterator>::Item
              type#4 trait#2::Item
              type#5 impl#5::Item = <I as Iterator>::Item
        "#,
        vec![TraitSelectionCase::probe(
            "prove nested adapter blanket impl",
            "Enumerate<Copied<Iter<User>>>: IntoIterator",
        )],
        expect![[r#"
            prove nested adapter blanket impl
              query: selection
              goal: Enumerate<Copied<Iter<User>>>: IntoIterator
              result: one
                impl: impl#5
                applicability: yes
        "#]],
    );
}

#[test]
fn probe_rejects_bare_inference_receiver_for_all_impl_shapes() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Marker
            structs
              struct#0 User
            impls
              impl#0 impl<T> Marker for T [resolved self: empty]
              impl#1 impl Marker for User
        "#,
        vec![TraitSelectionCase::probe(
            "reject bare inference receiver",
            "?receiver: Marker",
        )],
        expect![[r#"
            reject bare inference receiver
              query: selection
              goal: ?receiver: Marker
              result: empty
        "#]],
    );
}

#[test]
fn probe_rejects_concrete_self_mismatch() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 FromIterator<T>
            structs
              struct#0 Vec<T>
              struct#1 OtherVec<T>
              struct#2 User
            impls
              impl#0 impl<T> FromIterator<T> for Vec<T>
        "#,
        vec![TraitSelectionCase::probe(
            "reject mismatched self",
            "OtherVec<?item>: FromIterator<User>",
        )],
        expect![[r#"
            reject mismatched self
              query: selection
              goal: OtherVec<?item>: FromIterator<User>
              result: empty
        "#]],
    );
}

#[test]
fn probe_rejects_conflicting_repeated_type_param_evidence() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 FromIterator<T>
            structs
              struct#0 Vec<T>
              struct#1 User
              struct#2 Other
            impls
              impl#0 impl<T> FromIterator<T> for Vec<T>
        "#,
        vec![TraitSelectionCase::probe(
            "reject conflicting repeated type param",
            "Vec<User>: FromIterator<Other>",
        )],
        expect![[r#"
            reject conflicting repeated type param
              query: selection
              goal: Vec<User>: FromIterator<Other>
              result: empty
        "#]],
    );
}

#[test]
fn probe_keeps_multiple_applicable_impls_as_separate_candidates() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 FromIterator<T>
            structs
              struct#0 Vec<T>
              struct#1 User
            impls
              impl#0 impl<T> FromIterator<T> for Vec<T>
              impl#1 impl<T> FromIterator<T> for Vec<T>
              impl#2 impl FromIterator for <unsupported:unsupported self type> [resolved self: empty]
        "#,
        vec![TraitSelectionCase::probe(
            "keep multiple applicable impls ambiguous",
            "Vec<?item>: FromIterator<User>",
        )],
        expect![[r#"
            keep multiple applicable impls ambiguous
              query: selection
              goal: Vec<?item>: FromIterator<User>
              result: ambiguous
        "#]],
    );
}

#[test]
fn probe_rejects_impls_with_unproven_bounds() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Clone
              trait#1 FromIterator<T>
            structs
              struct#0 Vec<T>
              struct#1 User
            impls
              impl#0 impl<T: Clone> FromIterator<T> for Vec<T>
        "#,
        vec![TraitSelectionCase::probe(
            "reject unproven impl bound",
            "Vec<?item>: FromIterator<User>",
        )],
        expect![[r#"
            reject unproven impl bound
              query: selection
              goal: Vec<?item>: FromIterator<User>
              result: empty
        "#]],
    );
}

#[test]
fn probe_does_not_infer_unconstrained_impl_parameter_from_visible_impls() {
    // `T: Marker` constrains a type after another source establishes `T`; it is not an inverse
    // lookup from the set of Marker impls. Even a uniquely visible impl cannot make `T = User`,
    // because adding another Marker impl must not change inference at this call site.
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Marker
              trait#1 Target
            structs
              struct#0 Wrap<T>
              struct#1 User
            impls
              impl#0 impl Marker for User
              impl#1 impl<T: Marker> Target for Wrap<T>
        "#,
        vec![TraitSelectionCase::probe(
            "leave open impl parameter ambiguous",
            "Wrap<?item>: Target",
        )],
        expect![[r#"
            leave open impl parameter ambiguous
              query: selection
              goal: Wrap<?item>: Target
              result: one
                impl: impl#1
                applicability: maybe
                vars
                  ?item = _
        "#]],
    );
}

#[test]
fn probe_proves_impl_type_param_bounds() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Clone
              trait#1 FromIterator<T>
            structs
              struct#0 Vec<T>
              struct#1 User
            impls
              impl#0 impl<T: Clone> FromIterator<T> for Vec<T>
              impl#1 impl Clone for User
        "#,
        vec![TraitSelectionCase::probe(
            "prove concrete Clone bound",
            "Vec<?item>: FromIterator<User>",
        )],
        expect![[r#"
            prove concrete Clone bound
              query: selection
              goal: Vec<?item>: FromIterator<User>
              result: one
                impl: impl#0
                applicability: yes
                vars
                  ?item = User
        "#]],
    );
}

#[test]
fn probe_handles_visible_trait_data_with_generic_bounds() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Clone
              trait#1 NeedsClone<T: Clone>
              trait#2 FromIterator<T>
            structs
              struct#0 Vec<T>
              struct#1 User
            impls
              impl#0 impl<T: NeedsClone<T>> FromIterator<T> for Vec<T>
              impl#1 impl<T: Clone> NeedsClone<T> for T [resolved self: User]
              impl#2 impl Clone for User
        "#,
        vec![TraitSelectionCase::probe(
            "prove nested visible trait bounds",
            "Vec<?item>: FromIterator<User>",
        )],
        expect![[r#"
            prove nested visible trait bounds
              query: selection
              goal: Vec<?item>: FromIterator<User>
              result: one
                impl: impl#0
                applicability: yes
                vars
                  ?item = User
        "#]],
    );
}

#[test]
fn probe_declines_predicate_with_unsupported_bounded_associated_type() {
    // Associated type bounds need an additional Chalk binder layer that is not modeled yet. A
    // projection of such a type must stop at the lowering boundary instead of entering Chalk with
    // an ID whose associated-type datum was intentionally omitted.
    check_trait_selection_queries(
        r#"
            traits
              trait#0 LinkOps
              trait#1 Adapter
              trait#2 UsesAdapter
            structs
              struct#0 User
            impls
              impl#0 impl UsesAdapter for User where <User as Adapter>::LinkOps: LinkOps
            type aliases
              type#0 trait#1::LinkOps: LinkOps
        "#,
        vec![TraitSelectionCase::probe(
            "decline unsupported bounded associated type",
            "User: UsesAdapter",
        )],
        expect![[r#"
            decline unsupported bounded associated type
              query: selection
              goal: User: UsesAdapter
              result: empty
        "#]],
    );
}

#[test]
fn candidate_discovery_does_not_prove_impl_type_param_bounds() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Clone
              trait#1 FromIterator<T>
            structs
              struct#0 Vec<T>
              struct#1 User
            impls
              impl#0 impl<T: Clone> FromIterator<T> for Vec<T>
              impl#1 impl Clone for User
        "#,
        vec![
            TraitSelectionCase::probe(
                "default proves Clone bound",
                "Vec<?item>: FromIterator<User>",
            ),
            TraitSelectionCase::candidate_probe(
                "candidate leaves Clone bound unproved",
                "Vec<?item>: FromIterator<User>",
            ),
        ],
        expect![[r#"
            default proves Clone bound
              query: selection
              goal: Vec<?item>: FromIterator<User>
              result: one
                impl: impl#0
                applicability: yes
                vars
                  ?item = User

            candidate leaves Clone bound unproved
              query: candidate
              goal: Vec<?item>: FromIterator<User>
              result: one
                impl: impl#0
                applicability: yes
                vars
                  ?item = User
        "#]],
    );
}

#[test]
fn candidate_discovery_retains_impl_with_unproved_type_param_bound() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Clone
              trait#1 FromIterator<T>
            structs
              struct#0 Vec<T>
              struct#1 User
            impls
              impl#0 impl<T: Clone> FromIterator<T> for Vec<T>
        "#,
        vec![TraitSelectionCase::candidate_probe(
            "candidate retains unproved inline bound",
            "Vec<User>: FromIterator<User>",
        )],
        expect![[r#"
            candidate retains unproved inline bound
              query: candidate
              goal: Vec<User>: FromIterator<User>
              result: one
                impl: impl#0
                applicability: yes
        "#]],
    );
}
