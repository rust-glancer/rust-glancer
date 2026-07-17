mod utils;

use expect_test::expect;

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
fn chalk_proves_impl_predicate_associated_type_equality_constraints() {
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
fn chalk_defers_body_local_closure_projection() {
    // A closure's callable signature lives in Body IR. The shared Chalk database only has a stub
    // closure datum, so it must leave this projection for the body-aware obligation solver.
    check_trait_selection_queries(
        r#"
            traits
              trait#0 Iterator
              trait#1 FnOnce
            structs
              struct#0 Adapter<F>
            impls
              impl#0 impl<F: FnOnce> Iterator for Adapter<F>
            type aliases
              type#0 trait#0::Item
              type#1 trait#1::Output
              type#2 impl#0::Item = <F as FnOnce>::Output
        "#,
        vec![TraitSelectionCase::chalk_normalize_assoc(
            "defer body-local closure projection",
            "<Adapter<{closure#0}> as Iterator>::Item",
        )],
        expect![[r#"
            defer body-local closure projection
              query: chalk
              goal: <Adapter<{closure#0}> as Iterator>::Item
              result: none
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
fn candidate_discovery_exposes_impl_with_body_local_where_predicate() {
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
                "selection defers body-local predicate",
                "Adapter<{closure#0}>: Produces",
            ),
            TraitSelectionCase::candidate_probe(
                "candidate exposes canonical header",
                "Adapter<{closure#0}>: Produces",
            ),
        ],
        expect![[r#"
            selection defers body-local predicate
              query: selection
              goal: Adapter<{closure#0}>: Produces
              result: empty

            candidate exposes canonical header
              query: candidate
              goal: Adapter<{closure#0}>: Produces
              result: one
                impl: impl#0
                applicability: yes
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
