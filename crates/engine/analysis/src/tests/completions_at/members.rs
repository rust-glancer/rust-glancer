use std::fmt::Write as _;

use expect_test::expect;

use super::super::utils::{
    AnalysisQuery, check_analysis_queries, check_analysis_queries_with_fake_sysroot,
};
#[test]
fn completes_inherent_and_trait_methods_at_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Named {
    fn trait_name(&self);
    fn associated() {}
}

pub struct User;

impl User {
    pub fn new() -> Self {
        User
    }

    pub fn id(&self) {}

    pub fn touch(&mut self) {}
}

impl Named for User {
    fn trait_name(&self) {}
}

pub fn use_it(user: User) {
    user.$0id();
}
"#,
        &[AnalysisQuery::complete("dot completions", "0")],
        expect![[r#"
            dot completions
            - inherent_method id
            - inherent_method touch
            - trait_method trait_name
        "#]],
    );
}

#[test]
fn completes_methods_at_bare_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_bare_dot_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Named {
    fn trait_name(&self);
}

pub struct User;

impl User {
    pub fn id(&self) {}

    pub fn touch(&mut self) {}
}

impl Named for User {
    fn trait_name(&self) {}
}

pub fn use_it(user: User) {
    user.$0;
}
"#,
        &[AnalysisQuery::complete("bare dot completions", "0")],
        expect![[r#"
            bare dot completions
            - inherent_method id
            - inherent_method touch
            - trait_method trait_name
        "#]],
    );
}

#[test]
fn completion_ignores_unrelated_impls_in_speculative_trait_budget() {
    let mut fixture = String::from(
        r#"
//- /Cargo.toml
[package]
name = "analysis_trait_budget_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Rel<T> {}

pub struct User;

impl User {
    pub fn marker(&self) {}
}

pub struct Source;

impl Rel<User> for Source {}
"#,
    );
    for index in 0..65 {
        writeln!(
            &mut fixture,
            "pub struct Other{index};\nimpl Rel<User> for Other{index} {{}}"
        )
        .expect("string writes should not fail");
    }
    fixture.push_str(
        r#"
pub fn infer<T>(_: impl Rel<T>) -> T {
    loop {}
}

pub fn inspect() {
    let value = infer(Source);
    value.$receiver$
}
"#,
    );

    check_analysis_queries(
        &fixture,
        &[AnalysisQuery::complete(
            "inferred receiver with many unrelated impls",
            "receiver",
        )],
        expect![[r#"
            inferred receiver with many unrelated impls
            - inherent_method marker
        "#]],
    );
}

#[test]
fn completes_bare_dot_before_following_statement() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_bare_dot_before_statement_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User {
    pub name: String,
}

impl User {
    pub fn id(&self) {}
}

pub fn use_it(user: User) {
    user.$0

    user.id();
}
"#,
        &[AnalysisQuery::complete_verbose(
            "bare dot before statement completions",
            "0",
        )],
        expect![[r#"
            bare dot before statement completions
            - inherent_method id
              detail: pub fn id(&self)
              sort: id|01|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(1) })
              replace: 119..119
              snippet: id()$0
            - field name
              detail: pub name: String
              sort: name|00|00|Field(FieldRef { owner: TypeDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: Struct(StructId(0)) }, index: 0 })
              replace: 119..119
        "#]],
    );
}

#[test]
fn completes_through_references_try_and_await_wrappers() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_wrapper_completion"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Result<T, E> {
    Ok(T),
    Err(E),
}

pub struct Error;

pub struct User {
    profile: Profile,
}

impl User {
    pub fn id(&self) {}
}

pub struct Profile;

pub fn load_user() -> Result<User, Error> {
    todo!()
}

pub async fn load_user_async() -> User {
    User { profile: Profile }
}

pub async fn use_it(user: User) -> Result<(), Error> {
    let raw = 0;
    let shared: &&User = &&user;
    (&user).$reference$;
    shared.$double_reference$;
    load_user()?.$try$;
    load_user_async().await.$await$;
    (raw as User).$cast$;
    Result::Ok(())
}
"#,
        &[
            AnalysisQuery::complete("reference completions", "reference"),
            AnalysisQuery::complete("double reference completions", "double_reference"),
            AnalysisQuery::complete("try completions", "try"),
            AnalysisQuery::complete("await completions", "await"),
            AnalysisQuery::complete("cast completions", "cast"),
        ],
        expect![[r#"
            reference completions
            - inherent_method id
            - field profile

            double reference completions
            - inherent_method id
            - field profile

            try completions
            - inherent_method id
            - field profile

            await completions
            - inherent_method id
            - field profile

            cast completions
            - inherent_method id
            - field profile
        "#]],
    );
}

#[test]
fn completes_methods_for_bin_root_library_type() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_bin_completion"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[[bin]]
name = "analysis-bin-completion"
path = "src/main.rs"

//- /src/lib.rs
pub struct Api;

impl Api {
    pub fn ping(&self) {}
    pub fn work(&self) {}
}

//- /src/main.rs
fn main() {
    let api: analysis_bin_completion::Api = todo!();
    api.$0;
}
"#,
        &[AnalysisQuery::complete("bin root completions", "0").in_bin("analysis_bin_completion")],
        expect![[r#"
            bin root completions
            - inherent_method ping
            - inherent_method work
        "#]],
    );
}

#[test]
fn does_not_trigger_inside_method_arguments() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_dot_range"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

impl User {
    pub fn id(&self, _value: u8) {}

    pub fn touch(&self) {}
}

pub fn use_it(user: User) {
    user.id($inside_arg$0);
}
"#,
        &[AnalysisQuery::complete(
            "completion inside method argument",
            "inside_arg",
        )],
        expect![[r#"
            completion inside method argument
            - <none>
        "#]],
    );
}

#[test]
fn preserves_distinct_same_name_candidates() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_duplicates"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Named {
    fn label(&self);
}

pub trait Displayed {
    fn label(&self);
}

pub struct User;

impl User {
    pub fn label(&self) {}
}

impl Named for User {
    fn label(&self) {}
}

impl Displayed for User {
    fn label(&self) {}
}

pub fn use_it(user: User) {
    user.$0label();
}
"#,
        &[AnalysisQuery::complete("same-name completions", "0")],
        expect![[r#"
            same-name completions
            - inherent_method label
            - trait_method label
            - trait_method label
        "#]],
    );
}

#[test]
fn does_not_complete_concrete_impl_methods_for_wrong_generic_args() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_concrete_impl_completion"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Error;

pub struct Wrapper<T> {
    value: T,
}

impl<T> Wrapper<T> {
    pub fn generic(&self) {}
}

impl Wrapper<User> {
    pub fn user_only(&self) {}
}

pub trait UserOnlyTrait {
    fn trait_user_only(&self);
}

impl UserOnlyTrait for Wrapper<User> {
    fn trait_user_only(&self) {}
}

pub fn use_it(error: Wrapper<Error>) {
    error.$0;
}
"#,
        &[AnalysisQuery::complete(
            "wrong generic arg completions",
            "0",
        )],
        expect![[r#"
            wrong generic arg completions
            - inherent_method generic
            - field value
        "#]],
    );
}

#[test]
fn resolves_generic_trait_methods_and_rejects_unmet_bounds() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_generic_trait_completion"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub struct Wrapper<T> {
    value: T,
}

pub trait GenericNamed {
    fn generic_trait_name(&self);
}

impl<T> GenericNamed for Wrapper<T> {
    fn generic_trait_name(&self) {}
}

pub trait BoundNamed {
    fn bounded_trait_name(&self);
}

pub trait Required {}

impl<T> BoundNamed for Wrapper<T>
where
    T: Required,
{
    fn bounded_trait_name(&self) {}
}

pub fn use_it(wrapper: Wrapper<User>) {
    wrapper.$0;
}
"#,
        &[AnalysisQuery::complete("generic trait completions", "0")],
        expect![[r#"
            generic trait completions
            - trait_method generic_trait_name
            - field value
        "#]],
    );
}

#[test]
fn rejects_trait_impls_with_different_const_arguments() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_const_trait_completion"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Wrapper<const N: usize>;

pub trait Named {
    fn label(&self);
}

impl Named for Wrapper<1> {
    fn label(&self) {}
}

pub fn use_it(wrapper: Wrapper<2>) {
    wrapper.$0;
}
"#,
        &[AnalysisQuery::complete("const trait impl completions", "0")],
        expect![[r#"
            const trait impl completions
            - <none>
        "#]],
    );
}

#[test]
fn completes_methods_after_field_receiver() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_field_receiver_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Profile;

impl Profile {
    pub fn display(&self) {}
}

pub struct User {
    pub profile: Profile,
}

pub fn use_it(user: User) {
    user.profile.$0;
}
"#,
        &[AnalysisQuery::complete("field receiver completions", "0")],
        expect![[r#"
            field receiver completions
            - inherent_method display
        "#]],
    );
}

#[test]
fn completes_fields_at_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_field_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Profile;

pub struct User {
    pub profile: Profile,
}

pub fn use_it(user: User) {
    user.$0;
}
"#,
        &[AnalysisQuery::complete("field completions", "0")],
        expect![[r#"
            field completions
            - field profile
        "#]],
    );
}

#[test]
fn completes_body_local_struct_fields_at_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_field_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn use_it() {
    struct User {
        id: UserId,
        profile: Profile,
    }
    struct Pair(UserId, Profile);
    struct UserId;
    struct Profile;

    let user: User;
    user.$0;

    let pair: Pair;
    pair.$tuple$;
}
"#,
        &[
            AnalysisQuery::complete("body-local field completions", "0"),
            AnalysisQuery::complete("body-local tuple field completions", "tuple"),
        ],
        expect![[r#"
            body-local field completions
            - field id
            - field profile

            body-local tuple field completions
            - field 0
            - field 1
        "#]],
    );
}

#[test]
fn completes_through_core_deref() {
    check_analysis_queries_with_fake_sysroot(
        r#"
//- /Cargo.toml
[workspace]
members = ["app"]
resolver = "3"

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /app/src/lib.rs
use std::sync::Arc;

pub struct User {
    pub id: Id,
}

pub struct Id;
pub struct Label;

impl User {
    pub fn label(&self) -> Label {
        missing()
    }
}

pub fn use_it(user: Arc<User>) {
    user.$deref$;
}
"#,
        &[AnalysisQuery::complete("Deref completions", "deref").in_lib("app")],
        expect![[r#"
            Deref completions
            - field id
            - inherent_method label
        "#]],
    );
}

#[test]
fn completes_structural_slice_inherent_methods() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["core", "app"]
resolver = "3"

//- /core/Cargo.toml
[package]
name = "fake_core"
version = "0.1.0"
edition = "2024"

//- /core/src/lib.rs
impl<T> [T] {
    pub fn first_ref(&self) -> &T {
        missing()
    }

    pub fn len(&self) -> usize {
        missing()
    }
}

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
core = { package = "fake_core", path = "../core" }

//- /app/src/lib.rs
pub struct Package;

pub fn use_it(packages: &[Package], array: [Package; 3], array_ref: &[Package; 3]) {
    packages.$slice_methods$;
    array.$array_methods$;
    array_ref.$array_ref_methods$;
}
"#,
        &[
            AnalysisQuery::complete("slice method completions", "slice_methods").in_lib("app"),
            AnalysisQuery::complete("array method completions", "array_methods").in_lib("app"),
            AnalysisQuery::complete("array ref method completions", "array_ref_methods")
                .in_lib("app"),
        ],
        expect![[r#"
            slice method completions
            - inherent_method first_ref
            - inherent_method len

            array method completions
            - inherent_method first_ref
            - inherent_method len

            array ref method completions
            - inherent_method first_ref
            - inherent_method len
        "#]],
    );
}

#[test]
fn completes_primitive_inherent_methods_directly_and_through_deref() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["runtime", "app"]
resolver = "3"

//- /runtime/Cargo.toml
[package]
name = "runtime"
version = "0.1.0"
edition = "2024"

//- /runtime/src/lib.rs
#[lang = "deref"]
pub trait Project {
    #[lang = "deref_target"]
    type Target: ?Sized;

    fn project(&self) -> &Self::Target;
}

impl str {
    pub fn contains(&self, needle: &str) -> bool {
        missing()
    }
}

impl u32 {
    pub fn count_ones(self) -> u32 {
        missing()
    }
}

pub struct OwnedText;

impl Project for OwnedText {
    type Target = str;

    fn project(&self) -> &Self::Target {
        missing()
    }
}

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
runtime = { path = "../runtime" }

//- /app/src/lib.rs
use runtime::OwnedText;

pub fn use_it(owned: OwnedText, borrowed: &str, scalar: u32) {
    owned.$owned$;
    borrowed.$borrowed$;
    scalar.$scalar$;
}
"#,
        &[
            AnalysisQuery::complete("primitive method through Deref", "owned").in_lib("app"),
            AnalysisQuery::complete("primitive method through reference", "borrowed").in_lib("app"),
            AnalysisQuery::complete("scalar primitive method", "scalar").in_lib("app"),
        ],
        expect![[r#"
            primitive method through Deref
            - inherent_method contains
            - trait_method project

            primitive method through reference
            - inherent_method contains

            scalar primitive method
            - inherent_method count_ones
        "#]],
    );
}

#[test]
fn completes_methods_generated_by_associated_item_macros() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["runtime", "app"]
resolver = "3"

//- /runtime/Cargo.toml
[package]
name = "runtime"
version = "0.1.0"
edition = "2024"

//- /runtime/src/lib.rs
pub struct Label;

macro_rules! nested_integer_methods {
    () => {
        pub fn generated_nested(self) -> Label {
            missing()
        }
    };
}

macro_rules! integer_methods {
    () => {
        pub fn generated_count(self) -> u32 {
            missing()
        }

        nested_integer_methods!();
    };
}

impl u32 {
    integer_methods!();
}

macro_rules! trait_items {
    () => {
        fn generated_trait(&self) -> Label;
    };
}

pub trait GeneratedTrait {
    trait_items!();
}

impl GeneratedTrait for u32 {
    fn generated_trait(&self) -> Label {
        missing()
    }
}

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
runtime = { path = "../runtime" }

//- /app/src/lib.rs
use runtime::GeneratedTrait;

pub fn use_it(value: u32) {
    value.$methods$;
}
"#,
        &[AnalysisQuery::complete("macro-generated methods", "methods").in_lib("app")],
        expect![[r#"
            macro-generated methods
            - inherent_method generated_count
            - inherent_method generated_nested
            - trait_method generated_trait
        "#]],
    );
}

#[test]
fn completes_raw_pointer_inherent_methods_with_compiler_provided_bounds() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["runtime", "app"]
resolver = "3"

//- /runtime/Cargo.toml
[package]
name = "runtime"
version = "0.1.0"
edition = "2024"

//- /runtime/src/lib.rs
#[lang = "pointee_sized"]
pub trait PointeeSized {}

impl<T: PointeeSized> *const T {
    pub fn is_null(self) -> bool {
        missing()
    }
}

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
runtime = { path = "../runtime" }

//- /app/src/lib.rs
pub fn use_it(pointer: *const u8) {
    pointer.$pointer$;
}
"#,
        &[AnalysisQuery::complete("raw pointer methods", "pointer").in_lib("app")],
        expect![[r#"
            raw pointer methods
            - inherent_method is_null
        "#]],
    );
}

#[test]
fn completes_trait_methods_for_unkeyed_and_blanket_impl_receivers() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["runtime", "app"]
resolver = "3"

//- /runtime/Cargo.toml
[package]
name = "runtime"
version = "0.1.0"
edition = "2024"

//- /runtime/src/lib.rs
pub struct Label;

pub trait ScalarLabel {
    fn scalar_label(&self) -> Label;
}

impl ScalarLabel for u32 {
    fn scalar_label(&self) -> Label {
        missing()
    }
}

pub trait ArrayElement {
    type Element;

    fn array_element(&self) -> &Self::Element;
}

impl<T, const N: usize> ArrayElement for [T; N] {
    type Element = T;

    fn array_element(&self) -> &Self::Element {
        missing()
    }
}

pub trait ReferenceIdentity {
    fn reference_identity(self) -> Self;
}

impl<T> ReferenceIdentity for &T {
    fn reference_identity(self) -> Self {
        self
    }
}

pub trait BlanketLabel {
    fn blanket_label(&self) -> Label;
}

impl<T> BlanketLabel for T {
    fn blanket_label(&self) -> Label {
        missing()
    }
}

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
runtime = { path = "../runtime" }

//- /app/src/lib.rs
use runtime::{ArrayElement, BlanketLabel, ReferenceIdentity, ScalarLabel};

pub struct User;

pub fn use_it(scalar: u32, array: [User; 3], user: User, reference: &User) {
    scalar.$scalar$;
    array.$array$;
    user.$blanket$;
    reference.$reference$;
}
"#,
        &[
            AnalysisQuery::complete("primitive trait methods", "scalar").in_lib("app"),
            AnalysisQuery::complete("array trait methods", "array").in_lib("app"),
            AnalysisQuery::complete("blanket trait methods", "blanket").in_lib("app"),
            AnalysisQuery::complete("reference trait methods", "reference").in_lib("app"),
        ],
        expect![[r#"
            primitive trait methods
            - trait_method blanket_label
            - trait_method scalar_label

            array trait methods
            - trait_method array_element
            - trait_method blanket_label

            blanket trait methods
            - trait_method blanket_label

            reference trait methods
            - trait_method blanket_label
            - trait_method reference_identity
        "#]],
    );
}

#[test]
fn completes_body_local_impl_methods_at_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_impl_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User {
        id: GlobalId,
    }

    impl User {
        fn id(&self) -> GlobalId {
            missing()
        }

        fn associated() -> GlobalId {
            missing()
        }
    }

    let user: User;
    user.$0;
}
"#,
        &[AnalysisQuery::complete("body-local impl completions", "0")],
        expect![[r#"
            body-local impl completions
            - field id
            - inherent_method id
        "#]],
    );
}

#[test]
fn completes_body_local_impl_methods_for_target_types_at_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_target_impl_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it(id: GlobalId) {
    impl GlobalId {
        fn local(&self) -> GlobalId {
            missing()
        }
    }

    id.$0;
}
"#,
        &[AnalysisQuery::complete(
            "body-local target impl completions",
            "0",
        )],
        expect![[r#"
            body-local target impl completions
            - inherent_method local
        "#]],
    );
}

#[test]
fn completes_body_local_trait_impl_methods_for_target_types_at_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_target_trait_impl_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;
pub struct Label;

pub fn use_it(id: GlobalId) {
    trait Named {
        fn label(&self) -> Label;
        fn make() -> Label;
    }

    impl Named for GlobalId {
        fn label(&self) -> Label {
            missing()
        }

        fn make() -> Label {
            missing()
        }
    }

    id.$0;
}
"#,
        &[AnalysisQuery::complete(
            "body-local target trait impl completions",
            "0",
        )],
        expect![[r#"
            body-local target trait impl completions
            - trait_method label
        "#]],
    );
}

#[test]
fn completes_body_local_impl_methods_from_nested_blocks() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_nested_body_local_impl_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User {
        id: GlobalId,
    }

    {
        impl User {
            fn id(&self) -> GlobalId {
                missing()
            }
        }
    }

    let user: User;
    user.$0;
}
"#,
        &[AnalysisQuery::complete(
            "nested body-local impl completions",
            "0",
        )],
        expect![[r#"
            nested body-local impl completions
            - field id
            - inherent_method id
        "#]],
    );
}

#[test]
fn completes_parent_body_local_impl_methods_from_nested_body() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_nested_body_parent_impl_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User;

    impl User {
        fn id(&self) -> GlobalId {
            missing()
        }

        fn make() -> GlobalId {
            missing()
        }
    }

    fn helper(user: User) {
        user.$0;
    }
}
"#,
        &[AnalysisQuery::complete(
            "parent body-local impl completions from nested body",
            "0",
        )],
        expect![[r#"
            parent body-local impl completions from nested body
            - inherent_method id
        "#]],
    );
}

#[test]
fn completes_body_local_generic_impl_method_return_and_field_receivers() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_generic_impl_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    struct Id;
    struct Error;
    struct User {
        id: Id,
    }

    impl User {
        fn label(&self) {}
    }

    struct Wrapper<T> {
        value: T,
    }

    impl<U> Wrapper<U> {
        fn get(&self) -> U {
            missing()
        }
    }

    impl Wrapper<User> {
        fn user_only(&self) -> User {
            missing()
        }
    }

    let wrapper: Wrapper<User>;
    wrapper.get().$method_return$;
    wrapper.value.$field_receiver$;

    let error: Wrapper<Error>;
    error.$wrong_receiver$;
}
"#,
        &[
            AnalysisQuery::complete("generic method return completions", "method_return"),
            AnalysisQuery::complete("generic field receiver completions", "field_receiver"),
            AnalysisQuery::complete("wrong generic receiver completions", "wrong_receiver"),
        ],
        expect![[r#"
            generic method return completions
            - field id
            - inherent_method label

            generic field receiver completions
            - field id
            - inherent_method label

            wrong generic receiver completions
            - inherent_method get
            - field value
        "#]],
    );
}

#[test]
fn completes_fields_and_methods_after_enum_pattern_payloads() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_enum_pattern_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Id;

pub struct User {
    id: Id,
}

impl User {
    fn is_valid(&self) -> bool {
        true
    }

    fn label(&self) {}
}

pub enum Option<T> {
    Some(T),
    None,
}

pub fn use_it(maybe: Option<User>) {
    let Some(value) = maybe else { return; };
    value.$let_payload$;

    if let Some(found) = maybe && found.$if_rhs$is_valid() {
        found.$if_payload$;
    }

    while let Some(next) = maybe {
        next.$while_payload$;
    }

    match maybe {
        Some(user) if user.$match_guard$is_valid() => user.$match_payload$,
        None => {}
    }
}
"#,
        &[
            AnalysisQuery::complete("let pattern payload completions", "let_payload"),
            AnalysisQuery::complete("if let-chain rhs completions", "if_rhs"),
            AnalysisQuery::complete("if let pattern payload completions", "if_payload"),
            AnalysisQuery::complete("while let pattern payload completions", "while_payload"),
            AnalysisQuery::complete("match guard payload completions", "match_guard"),
            AnalysisQuery::complete("match pattern payload completions", "match_payload"),
        ],
        expect![[r#"
            let pattern payload completions
            - field id
            - inherent_method is_valid
            - inherent_method label

            if let-chain rhs completions
            - field id
            - inherent_method is_valid
            - inherent_method label

            if let pattern payload completions
            - field id
            - inherent_method is_valid
            - inherent_method label

            while let pattern payload completions
            - field id
            - inherent_method is_valid
            - inherent_method label

            match guard payload completions
            - field id
            - inherent_method is_valid
            - inherent_method label

            match pattern payload completions
            - field id
            - inherent_method is_valid
            - inherent_method label
        "#]],
    );
}

#[test]
fn completes_fields_and_methods_after_closure_params() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_closure_pattern_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Id;

pub struct User {
    id: Id,
}

impl User {
    fn is_valid(&self) -> bool {
        true
    }

    fn label(&self) {}
}

pub fn use_it() {
    let _closure = |user: User| user.$closure_payload$;
}
"#,
        &[AnalysisQuery::complete(
            "closure param payload completions",
            "closure_payload",
        )],
        expect![[r#"
            closure param payload completions
            - field id
            - inherent_method is_valid
            - inherent_method label
        "#]],
    );
}

#[test]
fn completes_tuple_fields_at_dot() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_tuple_field_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Left;
pub struct Right;

pub struct Pair(pub Left, pub Right);

pub fn use_it(pair: Pair) {
    pair.$0;
}
"#,
        &[AnalysisQuery::complete("tuple field completions", "0")],
        expect![[r#"
            tuple field completions
            - field 0
            - field 1
        "#]],
    );
}

#[test]
fn completes_dot_members_with_metadata_and_replacement_range() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_metadata"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Profile;

pub struct User {
    /// Name field.
    pub name: Profile,
}

impl User {
    /// Name method.
    pub fn name(&self) -> Profile {
        todo!()
    }
}

pub fn use_it(user: User) {
    user.na$0;
}
"#,
        &[AnalysisQuery::complete_verbose("metadata completions", "0")],
        expect![[r#"
            metadata completions
            - field name
              detail: pub name: Profile
              docs: Name field.
              sort: name|00|00|Field(FieldRef { owner: TypeDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: Struct(StructId(1)) }, index: 0 })
              replace: 216..218
            - inherent_method name
              detail: pub fn name(&self) -> Profile
              docs: Name method.
              sort: name|01|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(1) })
              replace: 216..218
              snippet: name()$0
        "#]],
    );
}
