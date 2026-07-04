mod utils;

use expect_test::expect;

use super::TraitSelectionOptions;

use self::utils::*;

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
              options: default
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
              options: default
              goal: <Iter<User> as Iterator>::Item
              result: projected
                infer: User
                final: User
                applicability: yes
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
        vec![
            TraitSelectionCase::probe("default selection", "Iter<User>: Iterator"),
            TraitSelectionCase::probe("exploratory selection", "Iter<User>: Iterator")
                .with_options(TraitSelectionOptions::new().keep_maybe_candidates()),
        ],
        expect![[r#"
            default selection
              options: default
              goal: Iter<User>: Iterator
              result: one
                impl: impl#0
                applicability: yes

            exploratory selection
              options: keep-maybe-candidates
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
              options: default
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
              options: default
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
              options: default
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
              options: default
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
fn normalize_assoc_type_projects_qualified_associated_type_value() {
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
              options: default
              goal: <Skip<Iter<User>> as Iterator>::Item
              result: projected
                infer: User
                final: User
                applicability: yes
        "#]],
    );
}

#[test]
fn blanket_self_param_impl_proves_bound_before_projecting_item() {
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
              type#3 impl#1::Item = I::Item
        "#,
        vec![
            TraitSelectionCase::probe("prove blanket iterator impl", "Iter<User>: IntoIterator"),
            TraitSelectionCase::normalize_assoc(
                "project blanket IntoIterator Item",
                "<Iter<User> as IntoIterator>::Item",
            ),
            TraitSelectionCase::probe(
                "prove blanket iterator impl for opaque iterator",
                "impl Iterator<Item = User>: IntoIterator",
            ),
            TraitSelectionCase::normalize_assoc(
                "project blanket opaque IntoIterator Item",
                "<impl Iterator<Item = User> as IntoIterator>::Item",
            ),
            TraitSelectionCase::probe(
                "reject unproved blanket iterator impl",
                "NotIter: IntoIterator",
            ),
        ],
        expect![[r#"
            prove blanket iterator impl
              options: default
              goal: Iter<User>: IntoIterator
              result: one
                impl: impl#1
                applicability: yes

            project blanket IntoIterator Item
              options: default
              goal: <Iter<User> as IntoIterator>::Item
              result: projected
                infer: User
                final: User
                applicability: yes

            prove blanket iterator impl for opaque iterator
              options: default
              goal: impl Iterator<Item = User>: IntoIterator
              result: one
                impl: impl#1
                applicability: yes

            project blanket opaque IntoIterator Item
              options: default
              goal: <impl Iterator<Item = User> as IntoIterator>::Item
              result: projected
                infer: User
                final: User
                applicability: yes

            reject unproved blanket iterator impl
              options: default
              goal: NotIter: IntoIterator
              result: empty
        "#]],
    );
}

#[test]
fn probe_rejects_bare_inference_var_blanket_self_match() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Marker
            impls
              impl#0 impl<T> Marker for T [resolved self: empty]
        "#,
        vec![TraitSelectionCase::probe(
            "reject bare inference receiver",
            "?receiver: Marker",
        )],
        expect![[r#"
            reject bare inference receiver
              options: default
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
              options: default
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
              options: default
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
              options: default
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
              options: default
              goal: Vec<?item>: FromIterator<User>
              result: empty
        "#]],
    );
}

#[test]
fn probe_uses_chalk_to_prove_impl_type_param_bounds() {
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
            "prove Clone bound with Chalk",
            "Vec<?item>: FromIterator<User>",
        )],
        expect![[r#"
            prove Clone bound with Chalk
              options: default
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
              options: default
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
fn caller_solved_impl_predicates_can_return_impl_with_where_predicates() {
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Produces
              trait#1 FnOnce
            structs
              struct#0 Adapter<F>
            impls
              impl#0 impl<F, R> Produces for Adapter<F> where F: FnOnce
        "#,
        vec![
            TraitSelectionCase::probe(
                "default rejects unsolved where predicate",
                "Adapter<{closure#0}>: Produces",
            ),
            TraitSelectionCase::probe(
                "caller solves impl predicate",
                "Adapter<{closure#0}>: Produces",
            )
            .with_options(TraitSelectionOptions::new().caller_solves_impl_predicates()),
        ],
        expect![[r#"
            default rejects unsolved where predicate
              options: default
              goal: Adapter<{closure#0}>: Produces
              result: empty

            caller solves impl predicate
              options: caller-solves-impl-predicates
              goal: Adapter<{closure#0}>: Produces
              result: one
                impl: impl#0
                applicability: yes
        "#]],
    );
}

#[test]
fn header_only_rejects_impl_type_param_bounds_even_when_chalk_can_prove_them() {
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
            TraitSelectionCase::probe(
                "header-only rejects Clone bound",
                "Vec<?item>: FromIterator<User>",
            )
            .with_options(TraitSelectionOptions::new().header_only()),
        ],
        expect![[r#"
            default proves Clone bound
              options: default
              goal: Vec<?item>: FromIterator<User>
              result: one
                impl: impl#0
                applicability: yes
                vars
                  ?item = User

            header-only rejects Clone bound
              options: header-only
              goal: Vec<?item>: FromIterator<User>
              result: empty
        "#]],
    );
}

#[test]
fn caller_solved_impl_predicates_can_return_impl_with_type_param_bounds() {
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
        vec![
            TraitSelectionCase::probe(
                "caller solves inline bound",
                "Vec<User>: FromIterator<User>",
            )
            .with_options(TraitSelectionOptions::new().caller_solves_impl_predicates()),
        ],
        expect![[r#"
            caller solves inline bound
              options: caller-solves-impl-predicates
              goal: Vec<User>: FromIterator<User>
              result: one
                impl: impl#0
                applicability: yes
        "#]],
    );
}
