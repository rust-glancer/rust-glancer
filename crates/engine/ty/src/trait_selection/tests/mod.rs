mod utils;

use std::fmt::Write as _;

use expect_test::expect;

use self::utils::*;
use crate::{AdtTy, AliasTy, GenericArg, Ty, TyContext, lookup::ImplQuery};

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
    let relevant_traits = lookup
        .traits_with_function_name("target")
        .expect("candidate lookup succeeds");
    assert_eq!(
        relevant_traits.as_slice(),
        &[fixture
            .trait_ref_by_name("Target")
            .expect("fixture should contain Target")]
    );

    let context = TyContext::new(
        &fixture,
        &fixture,
        lookup,
        fixture.target,
        rg_std::CancellationToken::new(),
    );
    let impl_query = ImplQuery::new(context);
    let receiver_ty = Ty::adt(AdtTy {
        def: fixture
            .type_ref_by_name("User")
            .expect("fixture should contain User"),
        args: Vec::new().into(),
    });
    let matches = impl_query
        .matches_for_receiver_with_traits(&receiver_ty, relevant_traits)
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
    assert_eq!(
        prove_fixture_goal(&fixture, "Box<?item>: Marker"),
        crate::solver::Outcome::Ambiguous
    );
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
    assert_eq!(
        prove_fixture_goal(&fixture, "Box<?item>: Marker"),
        crate::solver::Outcome::Ambiguous
    );
}

#[test]
fn probe_matches_direct_generic_impl_evidence() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 FromIterator<T>
            structs
              struct#0 Vec<T>
              struct#1 OtherVec<T>
              struct#2 User
              struct#3 Other
            impls
              impl#0 impl<T> FromIterator<T> for Vec<T>
        "#,
        vec![
            TraitSelectionCase::probe("select direct impl", "Vec<?item>: FromIterator<User>"),
            TraitSelectionCase::probe(
                "reject mismatched self",
                "OtherVec<?item>: FromIterator<User>",
            ),
            TraitSelectionCase::probe(
                "reject conflicting repeated type param",
                "Vec<User>: FromIterator<Other>",
            ),
        ],
        expect![[r#"
            select direct impl
              query: selection
              goal: Vec<?item>: FromIterator<User>
              result: one
                impl: impl#0
                applicability: yes
                vars
                  ?item = User

            reject mismatched self
              query: selection
              goal: OtherVec<?item>: FromIterator<User>
              result: empty

            reject conflicting repeated type param
              query: selection
              goal: Vec<User>: FromIterator<Other>
              result: empty
        "#]],
    );
}

#[test]
fn normalizes_generic_associated_types_with_live_variables() {
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
        vec![
            TraitSelectionCase::normalize_assoc(
                "project concrete generic impl Item",
                "<Iter<User> as Iterator>::Item",
            ),
            TraitSelectionCase::normalize_assoc(
                "preserve unresolved projection variable",
                "<Iter<?item> as Iterator>::Item",
            ),
        ],
        expect![[r#"
            project concrete generic impl Item
              query: selection
              goal: <Iter<User> as Iterator>::Item
              result: projected
                final: User
                applicability: yes

            preserve unresolved projection variable
              query: selection
              goal: <Iter<?item> as Iterator>::Item
              result: projected
                final: _
                applicability: yes
                vars
                  ?item = _
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
fn inherited_equalities_preserve_supertrait_arguments() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Base<T>
              trait#1 Derived<T>: Base<Vec<T>>
              trait#2 Further<T>: Derived<Option<T>>
            structs
              struct#0 Vec<T>
              struct#1 Option<T>
              struct#2 Producer<T>
              struct#3 User
            impls
              impl#0 impl<T> Base<Vec<T>> for Producer<T>
              impl#1 impl<T> Derived<T> for Producer<T>
              impl#2 impl<T> Further<T> for Producer<Option<T>>
            type aliases
              type#0 trait#0::Item
              type#1 impl#0::Item = T
            functions
              fn#0 opaque -> impl Further<User, Item = Option<User>>
        "#,
        vec![
            TraitSelectionCase::normalize_assoc(
                "inherited projection",
                "<Producer<User> as Derived<User>>::Item",
            ),
            TraitSelectionCase::normalize_assoc(
                "two supertrait substitutions",
                "<Producer<Option<User>> as Further<User>>::Item",
            ),
            TraitSelectionCase::probe(
                "equality constrains live projection arguments",
                "Producer<?item>: Further<?arg, Item = Option<User>>",
            ),
            TraitSelectionCase::normalize_assoc(
                "opaque equality keeps transformed arguments",
                "<opaque#0 as Base<Vec<Option<User>>>>::Item",
            ),
        ],
        expect![[r#"
            inherited projection
              query: selection
              goal: <Producer<User> as Derived<User>>::Item
              result: projected
                final: User
                applicability: yes

            two supertrait substitutions
              query: selection
              goal: <Producer<Option<User>> as Further<User>>::Item
              result: projected
                final: Option<User>
                applicability: yes

            equality constrains live projection arguments
              query: selection
              goal: Producer<?item>: Further<?arg, Item = Option<User>>
              result: one
                impl: impl#2
                applicability: yes
                vars
                  ?arg = User
                  ?item = Option<User>

            opaque equality keeps transformed arguments
              query: selection
              goal: <impl Further<User, Item = Option<User>> as Base<Vec<Option<User>>>>::Item
              result: projected
                final: Option<User>
                applicability: yes
        "#]],
    );
}

#[test]
fn solver_resolves_impl_predicate_associated_type_equality_constraints() {
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
        vec![TraitSelectionCase::probe(
            "default selection",
            "Iter<User>: Iterator",
        )],
        expect![[r#"
            default selection
              query: selection
              goal: Iter<User>: Iterator
              result: one
                impl: impl#0
                applicability: yes
        "#]],
    );
}

#[test]
fn solver_commits_projection_answer_evidence_to_inference_table() {
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
        vec![TraitSelectionCase::normalize_assoc(
            "solver solves projection variable",
            "<Vec<?item> as Indexed<User>>::Item",
        )],
        expect![[r#"
            solver solves projection variable
              query: selection
              goal: <Vec<?item> as Indexed<User>>::Item
              result: projected
                final: User
                applicability: yes
                vars
                  ?item = User
        "#]],
    );
}

#[test]
fn solver_raises_associated_type_constructor_shapes() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Shapes
            structs
              struct#0 Holder<T>
              struct#1 User
            impls
              impl#0 impl<T> Shapes for Holder<T>
            type aliases
              type#0 trait#0::Array
              type#1 impl#0::Array = [T; 3]
              type#2 trait#0::Pointer
              type#3 impl#0::Pointer = *const T
              type#4 trait#0::Callback
              type#5 impl#0::Callback = fn(T) -> T
        "#,
        vec![
            TraitSelectionCase::normalize_assoc(
                "solver projects array value",
                "<Holder<?item> as Shapes>::Array",
            ),
            TraitSelectionCase::normalize_assoc(
                "solver projects raw pointer",
                "<Holder<User> as Shapes>::Pointer",
            ),
            TraitSelectionCase::normalize_assoc(
                "solver projects function pointer",
                "<Holder<User> as Shapes>::Callback",
            ),
        ],
        expect![[r#"
            solver projects array value
              query: selection
              goal: <Holder<?item> as Shapes>::Array
              result: projected
                final: [_; 3]
                applicability: yes
                vars
                  ?item = _

            solver projects raw pointer
              query: selection
              goal: <Holder<User> as Shapes>::Pointer
              result: projected
                final: *const User
                applicability: yes

            solver projects function pointer
              query: selection
              goal: <Holder<User> as Shapes>::Callback
              result: projected
                final: fn(User) -> User
                applicability: yes
        "#]],
    );
}

#[test]
fn normalize_assoc_type_recurses_to_terminal_answer() {
    // The selected solver datum first produces `I::Item`. Recursive normalization feeds that
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
                final: User
                applicability: yes
        "#]],
    );
}

#[test]
fn blanket_self_param_impl_and_source_opaque_bounds_are_proved() {
    // Pair blanket-impl selection with solver's terminal associated value for both a nominal
    // iterator and an opaque iterator. Opaque equality comes from its declared solver datum rather
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
fn impl_bounds_distinguish_proven_unproved_and_candidate_queries() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Clone
              trait#1 FromIterator<T>
            structs
              struct#0 Vec<T>
              struct#1 User
              struct#2 Other
            impls
              impl#0 impl<T: Clone> FromIterator<T> for Vec<T>
              impl#1 impl Clone for User
        "#,
        vec![
            TraitSelectionCase::probe(
                "prove concrete Clone bound",
                "Vec<?item>: FromIterator<User>",
            ),
            TraitSelectionCase::probe(
                "reject unproved Clone bound",
                "Vec<?item>: FromIterator<Other>",
            ),
        ],
        expect![[r#"
            prove concrete Clone bound
              query: selection
              goal: Vec<?item>: FromIterator<User>
              result: one
                impl: impl#0
                applicability: yes
                vars
                  ?item = User

            reject unproved Clone bound
              query: selection
              goal: Vec<?item>: FromIterator<Other>
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
    // A bound on an associated type cannot provide the missing impl for its declaring trait.
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
fn unavailable_candidate_does_not_change_independent_selection_or_normalization() {
    use rg_ir_model::{ImplId, ImplRef};
    use rg_std::ExpectedUnique;

    use crate::solver::{self, Outcome};

    let fixture = TraitSelectionFixture::new(
        r#"
        traits
          trait#0 Iterator
        structs
          struct#0 User
        impls
          impl#0 impl Iterator for User
        type aliases
          type#0 trait#0::Item
          type#1 impl#0::Item = bool
    "#,
    );
    let context = TyContext::new(
        &fixture,
        &fixture,
        fixture.lookup_query(),
        fixture.target,
        rg_std::CancellationToken::new(),
    );
    solver::SemanticDeclarations::new(&context, context.item_paths())
        .with_solver(|solver| {
            let table = solver::InferenceTable::new(solver, Default::default());
            let cx = table.interner();
            let receiver = cx.lower_ty(
                &Ty::adt(AdtTy {
                    def: fixture.type_ref_by_name("User").expect("fixture User"),
                    args: Default::default(),
                }),
                &[],
            );
            let application = solver::TraitApplication {
                def: fixture
                    .trait_ref_by_name("Iterator")
                    .expect("fixture Iterator"),
                args: solver::List::new(cx, &[receiver.into()]),
            };
            let valid = ImplRef {
                origin: origin(),
                id: ImplId(0),
            };
            let missing = ImplRef {
                origin: origin(),
                id: ImplId(99),
            };
            let item = fixture
                .associated_ty_by_name(application.def, "Item")
                .expect("fixture Item");
            for candidates in [[missing, valid], [valid, missing]] {
                // The missing declaration returns from header matching before fulfillment.
                // Neither candidate order may hide the valid impl or spoil later normalization.
                let ExpectedUnique::One(selected) =
                    table.select_trait_impl(application, &[], candidates)
                else {
                    panic!("the available impl remains uniquely selected");
                };
                assert_eq!(selected.impl_ref, valid);
                assert_eq!(selected.outcome, Outcome::Proven);
                let (normalized, outcome) = table
                    .normalize_assoc_type(
                        application,
                        &[],
                        solver::ProjectionTy {
                            associated_ty: item,
                            args: application.args,
                        },
                    )
                    .expect("independent normalization succeeds");
                assert_eq!(outcome, Outcome::Proven);
                assert_eq!(
                    table.finalize(normalized),
                    Ty::Primitive(crate::PrimitiveTy::Bool)
                );
            }
            // An unavailable environment remains an unavailable input to its own table; it
            // must neither become valid on the second proof nor poison a different table.
            let missing_owner = solver::DefId::Function(rg_ir_model::FunctionRef {
                origin: origin(),
                id: rg_ir_model::FunctionId(99),
            });
            let incomplete = solver::InferenceTable::new(
                solver::Solver::new(cx),
                cx.parameter_environment(missing_owner),
            );
            for _ in 0..2 {
                assert_eq!(
                    incomplete.prove([application.clause(cx)]),
                    Outcome::Unavailable
                );
                assert_eq!(table.prove([application.clause(cx)]), Outcome::Proven);
            }
        })
        .expect("fixture declarations load");
}

#[test]
fn unavailable_nested_projection_rolls_back_its_root_and_leaves_other_roots_usable() {
    use rustc_type_ir::{InferCtxtLike, Upcast};

    use crate::solver::{self, Outcome};

    let fixture = TraitSelectionFixture::new(
        r#"
        traits
          trait#0 Iterator
          trait#1 Marker<T>
          trait#2 Copy
        structs
          struct#0 Incomplete
          struct#1 Complete
        impls
          impl#0 impl Iterator for Incomplete
          impl#1 impl Marker<bool> for Incomplete where <Incomplete as Iterator>::Item: Copy
          impl#2 impl Marker<char> for Complete
        type aliases
          type#0 trait#0::Item
    "#,
    );
    let context = TyContext::new(
        &fixture,
        &fixture,
        fixture.lookup_query(),
        fixture.target,
        rg_std::CancellationToken::new(),
    );
    solver::SemanticDeclarations::new(&context, context.item_paths())
        .with_solver(|solver| {
            let cx = solver.interner();
            let receivers = ["Incomplete", "Complete"].map(|name| {
                cx.lower_ty(
                    &Ty::adt(AdtTy {
                        def: fixture.type_ref_by_name(name).expect("fixture receiver"),
                        args: Default::default(),
                    }),
                    &[],
                )
            });
            let marker = fixture.trait_ref_by_name("Marker").expect("fixture Marker");
            // Matching the impl can learn ?T = bool before the nested Item projection fails.
            // Repeat the root to check that its incomplete answer was not kept in the cache.
            for _ in 0..2 {
                let variable = solver.next_ty_infer();
                for (receiver, expected) in receivers
                    .into_iter()
                    .zip([Outcome::Unavailable, Outcome::Proven])
                {
                    let goal = solver::TraitApplication {
                        def: marker,
                        args: solver::List::new(cx, &[receiver.into(), variable.into()]),
                    };
                    assert_eq!(
                        solver.evaluate(Default::default(), goal.clause(cx).upcast(cx)),
                        expected
                    );
                }
                // Complete requires char, so it cannot succeed if the failed goal left bool
                // assigned to this variable.
                assert_eq!(
                    cx.raise_ty(solver.shallow_resolve(variable)),
                    Some(Ty::Primitive(crate::PrimitiveTy::Char)),
                );
            }

            let table = solver::InferenceTable::new(solver, Default::default());
            let variables = [table.new_type_var(), table.new_type_var()];
            for (receiver, variable) in receivers.into_iter().zip(variables) {
                table.register(
                    solver::TraitApplication {
                        def: marker,
                        args: solver::List::new(cx, &[receiver.into(), variable.into()]),
                    }
                    .clause(cx),
                );
            }
            assert_eq!(table.fulfill(), Outcome::Unavailable);
            let character = Ty::Primitive(crate::PrimitiveTy::Char);
            table
                .try_unify(variables[0], cx.lower_ty(&character, &[]))
                .expect("unavailable root leaves room for different evidence");
            assert_eq!(table.finalize(variables[0]), character);
            assert_eq!(table.finalize(variables[1]), character);
        })
        .expect("fixture declarations load");
}

#[test]
fn successful_probe_keeps_its_type_evidence_independent() {
    use crate::{PrimitiveTy, solver};

    let fixture = TraitSelectionFixture::new("");
    let context = TyContext::new(
        &fixture,
        &fixture,
        fixture.lookup_query(),
        fixture.target,
        rg_std::CancellationToken::new(),
    );
    solver::SemanticDeclarations::new(&context, context.item_paths())
        .with_solver(|solver| {
            let table = solver::InferenceTable::new(solver, Default::default());
            let cx = table.interner();
            let variable = table.new_type_var();
            let boolean = Ty::Primitive(PrimitiveTy::Bool);
            let character = Ty::Primitive(PrimitiveTy::Char);

            // A successful candidate can learn bool without preventing the caller from
            // accepting different evidence. Each must retain its own answer afterwards.
            let trial = table.probe();
            trial
                .try_unify(variable, cx.lower_ty(&boolean, &[]))
                .expect("trial accepts bool");
            table
                .try_unify(variable, cx.lower_ty(&character, &[]))
                .expect("parent independently accepts char");
            assert_eq!(trial.finalize(variable), boolean);
            assert_eq!(table.finalize(variable), character);
        })
        .expect("fixture declarations load");
}

#[test]
fn signature_parameter_lists_preserve_order_and_repetition() {
    use crate::{PrimitiveTy, solver};

    let fixture = TraitSelectionFixture::new("");
    let context = TyContext::new(
        &fixture,
        &fixture,
        fixture.lookup_query(),
        fixture.target,
        rg_std::CancellationToken::new(),
    );
    solver::SemanticDeclarations::new(&context, context.item_paths())
        .with_solver(|solver| {
            let cx = solver.interner();
            let boolean = cx.lower_ty(&Ty::Primitive(PrimitiveTy::Bool), &[]);
            let character = cx.lower_ty(&Ty::Primitive(PrimitiveTy::Char), &[]);
            let inputs = [boolean, character, character, boolean];
            let signature = cx.fn_pointer(&inputs, character);
            let solver::TyShape::FnPointer { params, .. } = signature.shape() else {
                panic!("function pointer retains its signature");
            };
            // A signature's parameter view must compare like an independently constructed
            // list, preserving repetitions and distinguishing a different order.
            assert_eq!(params.as_slice(), inputs);
            assert_eq!(params, solver::List::new(cx, &inputs));
            assert_ne!(
                params,
                solver::List::new(cx, &[boolean, boolean, character, character])
            );
        })
        .expect("fixture declarations load");
}

#[test]
fn inherited_equalities_survive_owned_substitution_and_cached_queries() {
    use rg_ir_model::{FunctionId, FunctionRef, Path, TraitApplicability};

    use crate::{
        Substitution,
        lookup::ItemPathQuery,
        lowering::{TypeLoweringAnchor, TypePathResolver},
        signature::SemanticSignatureQuery,
        solver,
        trait_selection::{TraitGoal, TraitSelectionQuery},
    };

    struct CachedScope<'a> {
        paths: ItemPathQuery<'a, &'a TraitSelectionFixture, &'a TraitSelectionFixture>,
        cache: solver::DeclarationCache,
    }
    impl TypePathResolver for CachedScope<'_> {
        type Error = std::convert::Infallible;

        fn resolve_type_path(
            &self,
            anchor: TypeLoweringAnchor,
            path: &Path,
        ) -> Result<rg_semantic_ir::TypePathResolution, Self::Error> {
            TypePathResolver::resolve_type_path(&self.paths, anchor, path)
        }
    }
    impl solver::SolverScope for CachedScope<'_> {
        fn declaration_cache(&self) -> Option<&solver::DeclarationCache> {
            Some(&self.cache)
        }
    }

    let fixture = TraitSelectionFixture::new(
        r#"
            traits
              trait#0 Base<T>
              trait#1 Derived<T>: Base<Vec<T>>
              trait#2 Further<T>: Derived<Option<T>>
            structs
              struct#0 Vec<T>
              struct#1 Option<T>
              struct#2 User
            functions
              fn#0 factory<T> -> impl Further<T, Item = Option<T>>
            type aliases
              type#0 trait#0::Item
        "#,
    );
    let context = TyContext::new(
        &fixture,
        &fixture,
        fixture.lookup_query(),
        fixture.target,
        rg_std::CancellationToken::new(),
    );
    let signatures = SemanticSignatureQuery::new(&fixture, &fixture);
    let function = FunctionRef {
        origin: origin(),
        id: FunctionId(0),
    };
    let signature = signatures
        .function(function)
        .expect("signature loads")
        .expect("factory");
    let Ty::Alias(AliasTy::Opaque(opaque)) = signature.ret else {
        panic!("factory returns an opaque type");
    };
    let bounds = signatures
        .opaque_bounds(&opaque)
        .expect("bounds load")
        .expect("opaque bounds");
    let param = context
        .item_paths()
        .generics()
        .generics(function.into())
        .expect("factory generics")
        .param_by_name("T")
        .expect("factory T");
    let user = Ty::adt(AdtTy {
        def: fixture.type_ref_by_name("User").expect("User"),
        args: Default::default(),
    });
    let expected = Ty::adt(AdtTy {
        def: fixture.type_ref_by_name("Option").expect("Option"),
        args: vec![GenericArg::Type(Box::new(user.clone()))].into(),
    });
    let mut subst = Substitution::new();
    subst.push(param, GenericArg::Type(Box::new(user)));
    let bound = subst.apply_trait_ref(&bounds[0]);
    let goal = TraitGoal::from_lowering(bound);
    let scope = CachedScope {
        paths: context.item_paths().clone(),
        cache: Default::default(),
    };

    // Each query gets fresh solver storage. Repeating it imports the declaration clauses from
    // the shared owned cache, while the substituted bound also crosses the owned/live boundary.
    for _ in 0..2 {
        let query = TraitSelectionQuery::with_resolver(context.clone(), &scope);
        let result = query
            .normalize_assoc_type(&goal, "Item")
            .expect("normalization loads")
            .expect("inherited Item normalizes");
        assert_eq!(result.applicability, TraitApplicability::Yes);
        assert_eq!(result.ty, expected);
    }
}
