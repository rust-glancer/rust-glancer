mod utils;

use std::fmt::Write as _;

use expect_test::expect;

use self::utils::*;
use crate::{AdtTy, AliasTy, GenericArg, ProjectionTy, Ty, TyContext, lookup::ImplQuery};

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
                "selection projects generic impl Item",
                "<Iter<User> as Iterator>::Item",
            ),
            TraitSelectionCase::normalize_assoc(
                "solver projects generic impl Item",
                "<Iter<User> as Iterator>::Item",
            ),
            TraitSelectionCase::normalize_assoc(
                "solver preserves projection variable",
                "<Iter<?item> as Iterator>::Item",
            ),
        ],
        expect![[r#"
            selection projects generic impl Item
              query: selection
              goal: <Iter<User> as Iterator>::Item
              result: projected
                final: User
                applicability: yes

            solver projects generic impl Item
              query: selection
              goal: <Iter<User> as Iterator>::Item
              result: projected
                final: User
                applicability: yes

            solver preserves projection variable
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
fn next_solver_keeps_projection_evidence_in_the_live_context() {
    use rustc_type_ir::{self as ir, InferCtxtLike, Upcast};

    use crate::solver::{self, Outcome};
    let fixture = TraitSelectionFixture::new(
        r#"
        traits
          trait#0 Iterator
          trait#1 Copy
        structs
          struct#0 Iter<T>
          struct#1 User
        impls
          impl#0 impl<T> Iterator for Iter<T>
          impl#1 impl Copy for User
        type aliases
          type#0 trait#0::Item
          type#1 impl#0::Item = T
    "#,
    );
    let context = TyContext::new(
        &fixture,
        &fixture,
        fixture.lookup_query(),
        fixture.target,
        rg_std::CancellationToken::new(),
    );
    let declarations = solver::SemanticDeclarations::new(&context, context.item_paths());
    let storage = solver::SolverStorage::new(&declarations);
    let cx = storage.interner();
    let solver = solver::Solver::new(cx);
    let user = Ty::adt(AdtTy {
        def: fixture.type_ref_by_name("User").expect("fixture User"),
        args: Vec::new().into(),
    });
    let iterator = cx.lower_ty(
        &Ty::adt(AdtTy {
            def: fixture.type_ref_by_name("Iter").expect("fixture Iter"),
            args: vec![GenericArg::Type(Box::new(user.clone()))].into(),
        }),
        &[],
    );
    let iterator_trait = fixture
        .trait_ref_by_name("Iterator")
        .expect("fixture Iterator");
    let item = solver.next_ty_infer();
    let args = solver::List::new(cx, &[iterator.into()]);
    let projection: solver::Predicate<'_> = ir::ProjectionPredicate {
        projection_term: ir::AliasTerm::new_from_args(
            cx,
            ir::AliasTermKind::ProjectionTy {
                def_id: solver::DefId::TypeAlias(
                    fixture
                        .associated_ty_by_name(iterator_trait, "Item")
                        .expect("fixture Item"),
                ),
            },
            args,
        ),
        term: item.into(),
    }
    .upcast(cx);
    let copy: solver::Predicate<'_> = ir::TraitRef::new_from_args(
        cx,
        solver::DefId::Trait(fixture.trait_ref_by_name("Copy").expect("fixture Copy")),
        solver::List::new(cx, &[item.into()]),
    )
    .upcast(cx);
    // The solver starts with an unresolved shared item variable. Candidate probing can use it
    // and roll back, while accepted normalization provides evidence to the dependent Copy goal.
    {
        let _probe = solver.snapshot();
        assert_eq!(
            solver.evaluate(Default::default(), projection),
            Outcome::Proven
        );
        assert_eq!(
            cx.raise_ty(solver.resolve_vars_if_possible(item)),
            Some(user.clone())
        );
    }
    assert_eq!(solver.shallow_resolve(item), item);
    assert_eq!(
        solver.evaluate(Default::default(), projection),
        Outcome::Proven
    );
    assert_eq!(solver.evaluate(Default::default(), copy), Outcome::Proven);
    assert_eq!(
        cx.raise_ty(solver.resolve_vars_if_possible(item)),
        Some(user)
    );
    // A dependent goal can enter fulfillment before its projection has supplied any evidence.
    // Both roots use the body's table, so the next pass observes the newly resolved item.
    let table = solver::InferenceTable::new(solver::Solver::new(cx), Default::default());
    let queued_item = table.new_type_var();
    let bound: solver::Clause<'_> = ir::TraitRef::new_from_args(
        cx,
        solver::DefId::Trait(fixture.trait_ref_by_name("Copy").expect("fixture Copy")),
        solver::List::new(cx, &[queued_item.into()]),
    )
    .upcast(cx);
    table.register(bound);
    let alias = cx.lower_ty(
        &Ty::Alias(AliasTy::Projection(ProjectionTy {
            associated_ty: fixture
                .associated_ty_by_name(iterator_trait, "Item")
                .expect("fixture Item"),
            args: vec![GenericArg::Type(Box::new(
                cx.raise_ty(iterator).expect("stable iterator"),
            ))]
            .into(),
        })),
        &[],
    );
    table.unify(queued_item, alias);
    assert_eq!(table.fulfill(), Outcome::Proven);
    assert_eq!(
        table.finalize(queued_item),
        cx.raise_ty(iterator)
            .and_then(|ty| match ty {
                Ty::Adt(adt) => adt.args[0].as_ty().cloned(),
                _ => None,
            })
            .expect("iterator item")
    );
    assert!(declarations.take_error().is_none());
}

#[test]
fn next_solver_uses_a_generic_functions_declared_environment() {
    use rustc_type_ir::{self as ir, InferCtxtLike, Upcast};

    use crate::solver::{self, Outcome};
    let fixture = TraitSelectionFixture::new(
        r#"
        traits
          trait#0 Iterator
          trait#1 Copy
        functions
          fn#0 item<I: Iterator<Item = T>, T: Copy> -> T
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
    let declarations = solver::SemanticDeclarations::new(&context, context.item_paths());
    let storage = solver::SolverStorage::new(&declarations);
    let cx = storage.interner();
    let solver = solver::Solver::new(cx);
    let owner = rg_ir_model::FunctionRef {
        origin: origin(),
        id: rg_ir_model::FunctionId(0),
    };
    let generics = context
        .item_paths()
        .generics()
        .generics(rg_ir_model::GenericDefRef::Function(owner))
        .expect("fixture generics");
    let params = generics.iter().map(|p| p.param()).collect::<Vec<_>>();
    let rg_ir_model::GenericParamRef::Type(iter_param) = params[0] else {
        panic!("type parameter")
    };
    let rg_ir_model::GenericParamRef::Type(item_param) = params[1] else {
        panic!("type parameter")
    };
    let iterator = cx.lower_ty(&Ty::Param(iter_param), &params);
    let item = solver.next_ty_infer();
    let iterator_trait = fixture
        .trait_ref_by_name("Iterator")
        .expect("fixture Iterator");
    let projection = ir::ProjectionPredicate {
        projection_term: ir::AliasTerm::new_from_args(
            cx,
            ir::AliasTermKind::ProjectionTy {
                def_id: solver::DefId::TypeAlias(
                    fixture
                        .associated_ty_by_name(iterator_trait, "Item")
                        .expect("fixture Item"),
                ),
            },
            solver::List::new(cx, &[iterator.into()]),
        ),
        term: item.into(),
    }
    .upcast(cx);
    let env = cx.parameter_environment(solver::DefId::Function(owner));
    assert_eq!(solver.evaluate(env, projection), Outcome::Proven);
    let copy = ir::TraitRef::new_from_args(
        cx,
        solver::DefId::Trait(fixture.trait_ref_by_name("Copy").expect("fixture Copy")),
        solver::List::new(cx, &[item.into()]),
    )
    .upcast(cx);
    assert_eq!(solver.evaluate(env, copy), Outcome::Proven);
    assert_eq!(
        cx.raise_ty(solver.resolve_vars_if_possible(item)),
        Some(Ty::Param(item_param))
    );
}

#[test]
fn shared_inference_finalizes_numeric_links_and_nested_types() {
    use crate::solver;
    let fixture = TraitSelectionFixture::new("structs\n  struct#0 Vec<T>\n  struct#1 User\n");
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
            let general = table.new_type_var();
            let integer = table.new_integer_var();
            let float = table.new_float_var();
            table.unify(general, integer);
            let u64_ty = Ty::Primitive(crate::PrimitiveTy::UnsignedInt(crate::UnsignedIntTy::U64));
            table.unify(integer, cx.lower_ty(&u64_ty, &[]));
            assert_eq!(table.finalize(general), u64_ty);
            assert_eq!(
                table.finalize(float),
                Ty::Primitive(crate::PrimitiveTy::Float(crate::FloatTy::F64))
            );

            let item = table.new_type_var();
            let vector = cx.adt(solver::AdtTy {
                def: fixture.type_ref_by_name("Vec").expect("Vec"),
                args: solver::List::new(cx, &[item.into()]),
            });
            let user = Ty::adt(AdtTy::bare(fixture.type_ref_by_name("User").expect("User")));
            let trial = table.probe();
            trial.unify(item, cx.lower_ty(&user, &[]));
            assert_eq!(
                trial.finalize(vector),
                Ty::adt(AdtTy {
                    def: fixture.type_ref_by_name("Vec").expect("Vec"),
                    args: vec![GenericArg::Type(Box::new(user.clone()))].into()
                })
            );
            assert_eq!(table.finalize(item), Ty::Unknown);
            table.unify(item, cx.lower_ty(&user, &[]));
            // A failed later relation keeps the accepted type, matching body-inference behavior.
            assert!(
                table
                    .try_unify(
                        item,
                        cx.lower_ty(&Ty::Primitive(crate::PrimitiveTy::Bool), &[])
                    )
                    .is_err()
            );
            assert_eq!(table.finalize(item), user);
        })
        .expect("fixture declarations load");
}

#[test]
fn generic_argument_relations_retain_const_evidence() {
    use crate::solver;
    let fixture = TraitSelectionFixture::new("structs\n  struct#0 User\n");
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
            let ty = table.new_type_var();
            let len = table.new_const_var();
            let args = solver::List::new(cx, &[ty.into(), len.into()]);
            let expected: crate::GenericArgs = vec![
                GenericArg::Type(Box::new(Ty::Primitive(crate::PrimitiveTy::Bool))),
                GenericArg::Const(crate::ConstValue::Scalar(3)),
            ]
            .into();
            table
                .try_unify_args(args, cx.lower_args(&expected, &[]))
                .expect("generic arguments relate");
            assert_eq!(table.finalize_args(args), expected);
        })
        .expect("fixture declarations load");
}

#[test]
fn source_holes_are_consumed_before_owned_results_are_serialized() {
    use crate::solver;
    let fixture = TraitSelectionFixture::new("structs\n  struct#0 User\n");
    let context = TyContext::new(
        &fixture,
        &fixture,
        fixture.lookup_query(),
        fixture.target,
        rg_std::CancellationToken::new(),
    );
    let finalized = solver::SemanticDeclarations::new(&context, context.item_paths())
        .with_solver(|solver| {
            let table = solver::InferenceTable::new(solver, Default::default());
            let holes = solver::SourceTypeHoles::new(&table);
            let source = Ty::tuple(vec![
                holes.allocate(),
                Ty::Primitive(crate::PrimitiveTy::Bool),
            ]);
            assert!(wincode::serialize(&source).is_err());
            let scoped = holes.lower(&source, &[]);
            let expected = Ty::tuple(vec![
                Ty::Primitive(crate::PrimitiveTy::Char),
                Ty::Primitive(crate::PrimitiveTy::Bool),
            ]);
            table.unify(scoped, table.interner().lower_ty(&expected, &[]));
            table.finalize(scoped)
        })
        .expect("fixture declarations load");
    assert!(!finalized.has_source_hole());
    let encoded = wincode::serialize(&finalized).expect("owned type serializes");
    let decoded: Ty = wincode::deserialize(&encoded).expect("owned type reads back");
    assert_eq!(decoded, finalized);
}
