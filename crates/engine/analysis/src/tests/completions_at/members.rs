use std::fmt::Write as _;

use expect_test::expect;

use crate::tests::utils::{
    AnalysisQuery, check_analysis, check_analysis_queries,
    check_analysis_queries_with_fake_sysroot, check_analysis_with_fake_sysroot,
};

#[test]
fn balances_dot_member_origin_and_expected_type() {
    check_analysis(
        r#"
//- /src/lib.rs crate:member_relevance
struct Widget;
impl Widget {
    fn pick_z_inherent(&self) -> bool { false }
}

trait Available {
    fn pick_a_trait(&self) -> u32 { 0 }
}
impl Available for Widget {}

mod extensions {
    pub trait Extension {
        fn pick_b_extension(&self) -> u16 { 0 }
    }
    impl Extension for super::Widget {}
}

fn demo(widget: Widget) {
    widget.$0; // complete("untyped member list").matching("pick_")
    widget.pick_$0; // complete("in scope methods first").auto_import().matching("pick_")
    let _: u32 = widget.pick_$0(); // complete("written call matches the trait result").matching("pick_")
    let _: u16 = widget.pick_$0; // complete("matching extension is worth its import").auto_import().matching("pick_")
}
"#,
        expect![[r#"
            // untyped member list
            im pick_z_inherent
            tm pick_a_trait

            // in scope methods first
            im pick_z_inherent
            tm pick_a_trait
            tm pick_b_extension

            // written call matches the trait result
            tm pick_a_trait
            im pick_z_inherent

            // matching extension is worth its import
            tm pick_b_extension
            im pick_z_inherent
            tm pick_a_trait
        "#]],
    );
}

#[test]
fn finds_methods_from_trait_objects_without_importing_the_trait() {
    check_analysis(
        r#"
//- /src/lib.rs crate:object_members
pub mod api {
    pub trait Base<T> {
        type Item;
        fn parent(&self) -> Self::Item;
    }
    pub trait Service<T>: Base<T> {
        fn request(&self) -> T;
    }
    impl<T> dyn Service<T, Item = u16> + '_ {
        pub fn inspect(&self) -> u64 { 0 }
    }
}
pub type Service = dyn api::Service<u8, Item = u16>;
pub fn consume<T: api::Base<u8, Item = u16> + ?Sized>(value: &T) -> T::Item { loop {} }
pub fn run(value: &Service) {
    let $0results = (value.request(), value.parent(), value.inspect(), consume(value)); // hover("object method results")
    let callback = <Service as api::Base<u8>>::$0parent; // goto("qualified method declaration")
    value$0.$1; // goto_type("object trait declaration"), complete("object methods").members()
}
"#,
        expect![[r#"
            // object method results
            range: 16:9-16:16
            block:
              kind: variable
              signature:
                let results: (u8, u16, u64, u16)

            fn parent @ 4:12-4:18 // qualified method declaration
            trait Service @ 6:15-6:22 // object trait declaration

            // object methods
            im inspect
            tm parent
            tm request
        "#]],
    );
}

#[test]
fn finds_methods_from_caller_bounds_without_concrete_impls() {
    check_analysis_queries(
        r#"
//- /src/lib.rs crate:bound_members
pub mod api {
    pub trait Base<T> {
        fn render_base(&self) -> T;
    }
    pub trait Render: Base<u16> {
        fn render(&self) -> u32;
    }
}

pub fn inspect<T: api::Render>(value: T) {
    let $direct$direct = value.$render$render();
    let $inherited$inherited = value.$base$render_base();
    value.$members$;
}
"#,
        &[
            AnalysisQuery::ty("direct bound result", "direct"),
            AnalysisQuery::ty("supertrait result", "inherited"),
            AnalysisQuery::goto("direct declaration", "render"),
            AnalysisQuery::goto("supertrait declaration", "base"),
            AnalysisQuery::complete("completed bound methods", "members")
                .members()
                .matching("render"),
        ],
        expect![[r#"
            u32 // direct bound result
            u16 // supertrait result
            fn render @ 6:12-6:18 // direct declaration
            fn render_base @ 3:12-3:23 // supertrait declaration

            // completed bound methods
            tm render
            tm render_base
        "#]],
    );
}

#[test]
fn finds_members_through_generic_deref_bounds() {
    check_analysis_queries_with_fake_sysroot(
        r#"
//- /app/src/lib.rs crate:app root:app
pub struct Widget {
    pub value: u16,
}

impl Widget {
    pub fn value_len(&self) -> u32 { 0 }
}

pub fn inspect<P, Q>(value: &P)
where
    P: core::ops::Deref<Target = Q>,
    Q: core::ops::Deref<Target = Widget>,
{
    let $field$field = value.value;
    let $method$method = value.value_len();
    value.$members$;
}
"#,
        &[
            AnalysisQuery::ty("field through generic Deref chain", "field").in_lib("app"),
            AnalysisQuery::ty("method through generic Deref chain", "method").in_lib("app"),
            AnalysisQuery::complete("generic Deref members", "members")
                .members()
                .in_lib("app")
                .matching("value"),
        ],
        expect![[r#"
            u16 // field through generic Deref chain
            u32 // method through generic Deref chain

            // generic Deref members
            fd value
            im value_len
        "#]],
    );
}

#[test]
fn finds_methods_using_enclosing_trait_and_impl_bounds() {
    check_analysis_queries(
        r#"
//- /src/lib.rs crate:owner_bound_members
// There is no concrete Marker impl; each body relies on its enclosing bounds.
pub trait Marker {}
pub trait ViaMarker { fn from_marker(&self) -> bool; }
impl<T: Marker> ViaMarker for T {
    fn from_marker(&self) -> bool { true }
}
pub trait ViaRender<T> { fn from_render(&self) -> u16; }
impl<S: Render<T>, T: Marker> ViaRender<T> for S {
    fn from_render(&self) -> u16 { 0 }
}

pub fn direct<T: Marker>(value: T) {
    let $function_result$result = value.from_marker();
    value.$function_members$;
}

pub trait Render<T: Marker> {
    fn default_method(&self, value: T) {
        let $trait_result$result = value.from_marker();
        value.$trait_members$;
        let $self_result$result = self.from_render();
        self.$self_members$;
    }
}

pub struct Other;
pub struct Owner;
impl Owner where Other: Marker {
    pub fn method(&self, value: Other) {
        let $impl_result$result = value.from_marker();
        value.$impl_members$;
    }
}
"#,
        &[
            AnalysisQuery::ty("function bound", "function_result"),
            AnalysisQuery::complete("completed function bound", "function_members")
                .members()
                .matching("from_marker"),
            AnalysisQuery::ty("trait parameter bound", "trait_result"),
            AnalysisQuery::complete("completed trait parameter bound", "trait_members")
                .members()
                .matching("from_marker"),
            AnalysisQuery::ty("implicit Self bound", "self_result"),
            AnalysisQuery::complete("completed implicit Self bound", "self_members")
                .members()
                .matching("from_render"),
            AnalysisQuery::ty("nongeneric impl bound", "impl_result"),
            AnalysisQuery::complete("completed nongeneric impl bound", "impl_members")
                .members()
                .matching("from_marker"),
        ],
        expect![[r#"
            bool // function bound
            tm from_marker // completed function bound
            bool // trait parameter bound
            tm from_marker // completed trait parameter bound
            u16 // implicit Self bound
            tm from_render // completed implicit Self bound
            bool // nongeneric impl bound
            tm from_marker // completed nongeneric impl bound
        "#]],
    );
}

#[test]
fn completes_inherent_and_trait_methods_at_partial_and_bare_dot_sites() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_completions
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
    user.$0id(); // complete("partial dot completions").members()
    user.$0; // complete("bare dot completions").members()
}
"#,
        expect![[r#"
            // partial dot completions
            im id
            im touch
            tm trait_name

            // bare dot completions
            im id
            im touch
            tm trait_name
        "#]],
    );
}

#[test]
fn trait_method_completion_respects_lexical_trait_scope() {
    check_analysis(
        r#"
//- /runtime/src/lib.rs crate:runtime root:runtime
pub struct Value;

pub mod named {
    pub trait Named { fn named(&self); }
    impl Named for super::Value { fn named(&self) {} }
}

pub mod alias {
    pub trait Aliased { fn aliased(&self); }
    impl Aliased for super::Value { fn aliased(&self) {} }
}

pub mod wildcard {
    pub trait Wildcard { fn wildcard(&self); }
    impl Wildcard for super::Value { fn wildcard(&self) {} }
}

pub mod underscore {
    pub trait Underscore { fn underscore(&self); }
    impl Underscore for super::Value { fn underscore(&self) {} }
}

pub mod hidden {
    pub trait Hidden { fn hidden(&self); }
    impl Hidden for super::Value { fn hidden(&self) {} }
}

//- /app/src/lib.rs crate:app root:app deps:runtime
use runtime::Value;
use runtime::alias::Aliased as Renamed;
use runtime::named::Named;
use runtime::underscore::Underscore as _;
use runtime::wildcard::*;

pub fn inspect(value: Value) {
    value.$0; // complete("outer trait scope").members().in_lib("app")
    {
        use runtime::hidden::Hidden;
        value.$0; // complete("block trait scope").members().in_lib("app")
    }
    {
        use runtime::hidden::Hidden as _;
        value.$0; // complete("block underscore trait scope").members().in_lib("app")
    }
    value.$0; // complete("scope after block").members().in_lib("app")
    {
        struct Named;
        value.$0; // complete("type shadow keeps outer trait").members().in_lib("app")
    }
}
"#,
        expect![[r#"
            // outer trait scope
            tm aliased
            tm named
            tm underscore
            tm wildcard

            // block trait scope
            tm aliased
            tm hidden
            tm named
            tm underscore
            tm wildcard

            // block underscore trait scope
            tm aliased
            tm hidden
            tm named
            tm underscore
            tm wildcard

            // scope after block
            tm aliased
            tm named
            tm underscore
            tm wildcard

            // type shadow keeps outer trait
            tm aliased
            tm named
            tm underscore
            tm wildcard
        "#]],
    );
}

#[test]
fn completion_ignores_unrelated_impls_in_speculative_trait_budget() {
    let mut fixture = String::from(
        r#"
//- /src/lib.rs crate:analysis_trait_budget_completions
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
        &[
            AnalysisQuery::complete("inferred receiver with many unrelated impls", "receiver")
                .members(),
        ],
        expect![[r#"
            im marker // inferred receiver with many unrelated impls
        "#]],
    );
}

#[test]
fn completes_bare_dot_before_following_statement() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_bare_dot_before_statement_completions
pub struct User {
    pub name: String,
}

impl User {
    pub fn id(&self) {}
}

pub fn use_it(user: User) {
    user.$0 // complete_verbose("bare dot before statement completions").members()

    user.id();
}
"#,
        expect![[r#"
            // bare dot before statement completions
            inherent_method id
              detail: pub fn id(&self)
              replace: 119..119
              snippet: id()$0
            field name
              detail: pub name: String
              replace: 119..119
        "#]],
    );
}

#[test]
fn completes_through_references_try_and_await_wrappers() {
    check_analysis_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:analysis_wrapper_completion
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
    (&user).$0; // complete("reference completions").members().in_lib("analysis_wrapper_completion")
    shared.$0; // complete("double reference completions").members().in_lib("analysis_wrapper_completion")
    load_user()?.$0; // complete("try completions").members().in_lib("analysis_wrapper_completion")
    load_user_async().await.$0; // complete("await completions").members().in_lib("analysis_wrapper_completion")
    (raw as User).$0; // complete("cast completions").members().in_lib("analysis_wrapper_completion")
    Result::Ok(())
}
"#,
        expect![[r#"
            // reference completions
            im id
            fd profile

            // double reference completions
            im id
            fd profile

            // try completions
            im id
            fd profile

            // await completions
            im id
            fd profile

            // cast completions
            im id
            fd profile
        "#]],
    );
}

#[test]
fn completes_methods_for_bin_root_library_type() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_bin_completion
pub struct Api;

impl Api {
    pub fn ping(&self) {}
    pub fn work(&self) {}
}

//- /src/main.rs crate:analysis_bin_completion kind:bin name:analysis-bin-completion
fn main() {
    let api: analysis_bin_completion::Api = todo!();
    api.$0; // complete("bin root completions").members().in_bin("analysis_bin_completion")
}
"#,
        expect![[r#"
            // bin root completions
            im ping
            im work
        "#]],
    );
}

#[test]
fn does_not_trigger_inside_method_arguments() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_completion_dot_range
pub struct User;

impl User {
    pub fn id(&self, _value: u8) {}

    pub fn touch(&self) {}
}

pub fn use_it(user: User) {
    user.id($0$0); // complete("completion inside method argument").members()
}
"#,
        expect![[r#"
            <none> // completion inside method argument
        "#]],
    );
}

#[test]
fn preserves_distinct_same_name_candidates() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_completion_duplicates
pub trait Named {
    fn label(&self);
}

pub trait Displayed {
    fn label(&self);
}

pub struct User;

impl User {
    pub fn label(&self) -> u32 { 0 }
}

impl Named for User {
    fn label(&self) {}
}

impl Displayed for User {
    fn label(&self) {}
}

pub fn use_it(user: User) {
    let $0picked = user.$1label(); // ty("inherent call takes precedence"), complete("same-name completions").members()
}
"#,
        expect![[r#"
            u32 // inherent call takes precedence

            // same-name completions
            im label
            tm label
            tm label
        "#]],
    );
}

#[test]
fn does_not_complete_concrete_impl_methods_for_wrong_generic_args() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_concrete_impl_completion
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
    error.$0; // complete("wrong generic arg completions").members()
}
"#,
        expect![[r#"
            // wrong generic arg completions
            im generic
            fd value
        "#]],
    );
}

#[test]
fn resolves_generic_trait_methods_and_rejects_unmet_bounds() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_generic_trait_completion
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
    wrapper.$0; // complete("generic trait completions").members()
}
"#,
        expect![[r#"
            // generic trait completions
            fd value
            tm generic_trait_name
        "#]],
    );
}

#[test]
fn rejects_trait_impls_with_different_const_arguments() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_const_trait_completion
pub struct Wrapper<const N: usize>;

pub trait Named {
    fn label(&self);
}

impl Named for Wrapper<1> {
    fn label(&self) {}
}

pub fn use_it(wrapper: Wrapper<2>) {
    wrapper.$0; // complete("const trait impl completions").members()
}
"#,
        expect![[r#"
            <none> // const trait impl completions
        "#]],
    );
}

#[test]
fn completes_members_for_nested_field_receivers() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_field_receiver_completions
pub struct Profile;

impl Profile {
    pub fn display(&self) {}
}

pub struct User {
    pub profile: Profile,
}

pub fn use_it(user: User) {
    user.profile.$0; // complete("field receiver completions").members()
    user.$0; // complete("outer receiver completions").members()
}
"#,
        expect![[r#"
            im display // field receiver completions
            fd profile // outer receiver completions
        "#]],
    );
}

#[test]
fn completes_body_local_struct_fields_at_dot() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_local_field_completions
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
    user.$0; // complete("body-local field completions").members()

    let pair: Pair;
    pair.$0; // complete("body-local tuple field completions").members()
}
"#,
        expect![[r#"
            // body-local field completions
            fd id
            fd profile

            // body-local tuple field completions
            fd 0
            fd 1
        "#]],
    );
}

#[test]
fn completes_through_core_deref() {
    check_analysis_with_fake_sysroot(
        r#"
//- /app/src/lib.rs crate:app root:app
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

pub fn use_it(user: Arc<User>, object: &dyn std::ops::Deref<Target = User>) {
    user.$0; // complete("Deref completions").members().in_lib("app")
    object.$0; // complete("dyn Deref completions").members().in_lib("app")
}
"#,
        expect![[r#"
            // Deref completions
            fd id
            im label

            // dyn Deref completions
            fd id
            im label
        "#]],
    );
}

#[test]
fn completes_structural_slice_inherent_methods() {
    check_analysis(
        r#"
//- /core/src/lib.rs crate:fake_core root:core
impl<T> [T] {
    pub fn first_ref(&self) -> &T {
        missing()
    }

    pub fn len(&self) -> usize {
        missing()
    }
}

//- /app/src/lib.rs crate:app root:app deps:core=fake_core
pub struct Package;

pub fn use_it(packages: &[Package], array: [Package; 3], array_ref: &[Package; 3]) {
    packages.$0; // complete("slice method completions").members().in_lib("app")
    array.$0; // complete("array method completions").members().in_lib("app")
    array_ref.$0; // complete("array ref method completions").members().in_lib("app")
}
"#,
        expect![[r#"
            // slice method completions
            im first_ref
            im len

            // array method completions
            im first_ref
            im len

            // array ref method completions
            im first_ref
            im len
        "#]],
    );
}

#[test]
fn completes_primitive_inherent_methods_directly_and_through_deref() {
    check_analysis(
        r#"
//- /runtime/src/lib.rs crate:runtime root:runtime
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

//- /app/src/lib.rs crate:app root:app deps:runtime
use runtime::OwnedText;

pub fn use_it(owned: OwnedText, borrowed: &str, scalar: u32) {
    owned.$0; // complete("primitive method through Deref").members().in_lib("app")
    borrowed.$0; // complete("primitive method through reference").members().in_lib("app")
    scalar.$0; // complete("scalar primitive method").members().in_lib("app")
}
"#,
        expect![[r#"
            im contains // primitive method through Deref
            im contains // primitive method through reference
            im count_ones // scalar primitive method
        "#]],
    );
}

#[test]
fn completes_methods_generated_by_associated_item_macros() {
    check_analysis(
        r#"
//- /runtime/src/lib.rs crate:runtime root:runtime
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

//- /app/src/lib.rs crate:app root:app deps:runtime
use runtime::GeneratedTrait;

pub fn use_it(value: u32) {
    value.$0; // complete("macro-generated methods").members().in_lib("app")
}
"#,
        expect![[r#"
            // macro-generated methods
            im generated_count
            im generated_nested
            tm generated_trait
        "#]],
    );
}

#[test]
fn completes_raw_pointer_inherent_methods_with_compiler_provided_bounds() {
    check_analysis(
        r#"
//- /runtime/src/lib.rs crate:runtime root:runtime
#[lang = "pointee_sized"]
pub trait PointeeSized {}

impl<T: PointeeSized> *const T {
    pub fn is_null(self) -> bool {
        missing()
    }
}

//- /app/src/lib.rs crate:app root:app deps:runtime
pub fn use_it(pointer: *const u8) {
    pointer.$0; // complete("raw pointer methods").members().in_lib("app")
}
"#,
        expect![[r#"
            im is_null // raw pointer methods
        "#]],
    );
}

#[test]
fn completes_trait_methods_for_unkeyed_and_blanket_impl_receivers() {
    check_analysis(
        r#"
//- /runtime/src/lib.rs crate:runtime root:runtime
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

//- /app/src/lib.rs crate:app root:app deps:runtime
use runtime::{ArrayElement, BlanketLabel, ReferenceIdentity, ScalarLabel};

pub struct User;

pub fn use_it(scalar: u32, array: [User; 3], user: User, reference: &User) {
    scalar.$0; // complete("primitive trait methods").members().in_lib("app")
    array.$0; // complete("array trait methods").members().in_lib("app")
    user.$0; // complete("blanket trait methods").members().in_lib("app")
    reference.$0; // complete("reference trait methods").members().in_lib("app")
}
"#,
        expect![[r#"
            // primitive trait methods
            tm blanket_label
            tm scalar_label

            // array trait methods
            tm array_element
            tm blanket_label

            tm blanket_label // blanket trait methods

            // reference trait methods
            tm blanket_label
            tm reference_identity
        "#]],
    );
}

#[test]
fn completes_body_local_impl_methods_across_scope_variants() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_local_impl_completion_matrix
pub struct GlobalId;
pub struct Label;

pub fn local_type_receiver() {
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
    user.$0; // complete("body-local type impl completions").members()
}

pub fn target_type_receiver(id: GlobalId) {
    impl GlobalId {
        fn local(&self) -> GlobalId {
            missing()
        }
    }

    id.$0; // complete("body-local target impl completions").members()
}

pub fn trait_target_type_receiver(id: GlobalId) {
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

    id.$0; // complete("body-local target trait impl completions").members()
}

pub fn nested_impl_block() {
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
    user.$0; // complete("nested body-local impl completions").members()
}

pub fn nested_body_owner() {
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
        user.$0; // complete("parent body-local impl completions from nested body").members()
    }
}
"#,
        expect![[r#"
            // body-local type impl completions
            fd id
            im id

            im local // body-local target impl completions
            tm label // body-local target trait impl completions

            // nested body-local impl completions
            fd id
            im id

            im id // parent body-local impl completions from nested body
        "#]],
    );
}

#[test]
fn completes_body_local_generic_impl_method_return_and_field_receivers() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_local_generic_impl_completions
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
    wrapper.get().$0; // complete("generic method return completions").members()
    wrapper.value.$0; // complete("generic field receiver completions").members()

    let error: Wrapper<Error>;
    error.$0; // complete("wrong generic receiver completions").members()
}
"#,
        expect![[r#"
            // generic method return completions
            fd id
            im label

            // generic field receiver completions
            fd id
            im label

            // wrong generic receiver completions
            im get
            fd value
        "#]],
    );
}

#[test]
fn completes_members_for_pattern_and_closure_introduced_receivers() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_enum_pattern_completions
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
    let _closure = |user: User| user.$0; // complete("closure parameter completions").members()

    let Some(value) = maybe else { return; };
    value.$0; // complete("let pattern payload completions").members()

    if let Some(found) = maybe && found.$0is_valid() { // complete("if let-chain rhs completions").members()
        found.$0; // complete("if let pattern payload completions").members()
    }

    while let Some(next) = maybe {
        next.$0; // complete("while let pattern payload completions").members()
    }

    match maybe {
        Some(user) if user.$0is_valid() => user.$1, // complete("match guard payload completions").members(), complete("match pattern payload completions").members()
        None => {}
    }
}
"#,
        expect![[r#"
            // closure parameter completions
            fd id
            im is_valid
            im label

            // let pattern payload completions
            fd id
            im is_valid
            im label

            // if let-chain rhs completions
            fd id
            im is_valid
            im label

            // if let pattern payload completions
            fd id
            im is_valid
            im label

            // while let pattern payload completions
            fd id
            im is_valid
            im label

            // match guard payload completions
            fd id
            im is_valid
            im label

            // match pattern payload completions
            fd id
            im is_valid
            im label
        "#]],
    );
}

#[test]
fn completes_tuple_fields_at_dot() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_tuple_field_completions
pub struct Left;
pub struct Right;

pub struct Pair(pub Left, pub Right);

pub fn use_it(pair: Pair) {
    pair.$0; // complete("tuple field completions").members()
}
"#,
        expect![[r#"
            // tuple field completions
            fd 0
            fd 1
        "#]],
    );
}

#[test]
fn completes_dot_members_with_metadata_and_replacement_range() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_completion_metadata
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
    user.na$0; // complete_verbose("metadata completions").members()
}
"#,
        expect![[r#"
            // metadata completions
            field name
              detail: pub name: Profile
              docs: Name field.
              replace: 216..218
            inherent_method name
              detail: pub fn name(&self) -> Profile
              docs: Name method.
              replace: 216..218
              snippet: name()$0
        "#]],
    );
}
