use expect_test::expect;

use super::utils::{check_analysis, check_analysis_with_fake_sysroot};

#[test]
fn returns_basic_body_expression_and_binding_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_type_at
pub struct User;

pub fn helper() -> User {
    User
}

pub fn use_it() {
    let local$0: User = helper()$1; // ty("binding declaration"), ty("binding initializer")
    let _typed: User = loc$0al; // ty("binding use")
}
"#,
        expect![[r#"
            nominal struct analysis_type_at[lib]::crate::User // binding declaration
            nominal struct analysis_type_at[lib]::crate::User // binding initializer
            nominal struct analysis_type_at[lib]::crate::User // binding use
        "#]],
    );
}

#[test]
fn returns_scalar_literal_and_operator_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_scalar_type_at
pub fn use_it(flag: bool, byte: u8, lhs: i32, rhs: i32) {
    let bool_lit = true$0; // ty("bool literal")
    let char_lit = 'x'$0; // ty("char literal")
    let byte_lit = b'x'$0; // ty("byte literal")
    let int_default = 1$0; // ty("default int literal")
    let int_suffix = 1usize$0; // ty("suffixed int literal")
    let float_default = 1.0$0; // ty("default float literal")
    let float_suffix = 1.0f32$0; // ty("suffixed float literal")
    let string_lit = "text"$0; // ty("string literal")
    let not_flag = (!flag)$0; // ty("not expression")
    let not_byte = (!byte)$0; // ty("integer not expression")
    let neg_lhs = (-lhs)$0; // ty("neg expression")
    let sum = (lhs + rhs)$0; // ty("sum expression")
    let compare = (lhs < rhs)$0; // ty("comparison expression")
    let logic = (flag && false)$0; // ty("logic expression")
    let bit = (lhs & rhs)$0; // ty("bit expression")
    let shift = (lhs << 1)$0; // ty("shift expression")
}
"#,
        expect![[r#"
            bool // bool literal
            char // char literal
            u8 // byte literal
            i32 // default int literal
            usize // suffixed int literal
            f64 // default float literal
            f32 // suffixed float literal
            &str // string literal
            bool // not expression
            u8 // integer not expression
            i32 // neg expression
            i32 // sum expression
            bool // comparison expression
            bool // logic expression
            i32 // bit expression
            i32 // shift expression
        "#]],
    );
}

#[test]
fn returns_types_for_references_try_and_await_wrappers() {
    check_analysis_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:analysis_wrapper_type_at
pub struct Error;
pub struct User;

pub fn load_user() -> Result<User, Error> {
    todo!()
}

pub async fn load_user_async() -> User {
    User
}

pub async fn use_it(mut user: User) -> Result<(), Error> {
    let _borrowed = (&user)$0; // ty("type at reference wrapper").in_lib("analysis_wrapper_type_at")
    let _borrowed_mut = (&mut user)$0; // ty("type at mutable reference wrapper").in_lib("analysis_wrapper_type_at")
    let typed_mut$0: &mut User = todo!(); // ty("type at mutable reference binding").in_lib("analysis_wrapper_type_at")
    let _loaded = load_user()?$0; // ty("type at try wrapper").in_lib("analysis_wrapper_type_at")
    let _borrowed_loaded = (&load_user())?$0; // ty("type at borrowed try wrapper").in_lib("analysis_wrapper_type_at")
    let _awaited = load_user_async().await$0; // ty("type at await wrapper").in_lib("analysis_wrapper_type_at")
    Result::Ok(())
}
"#,
        expect![[r#"
            &nominal struct analysis_wrapper_type_at[lib]::crate::User // type at reference wrapper
            &mut nominal struct analysis_wrapper_type_at[lib]::crate::User // type at mutable reference wrapper
            &mut nominal struct analysis_wrapper_type_at[lib]::crate::User // type at mutable reference binding
            nominal struct analysis_wrapper_type_at[lib]::crate::User // type at try wrapper
            <unknown> // type at borrowed try wrapper
            nominal struct analysis_wrapper_type_at[lib]::crate::User // type at await wrapper
        "#]],
    );
}

#[test]
fn autoderefs_references_for_member_lookup_and_explicit_deref() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_reference_autoderef
pub struct Profile;
pub struct Label;

pub struct User {
    pub profile: Profile,
}

impl User {
    pub fn label(&self) -> Label {
        missing()
    }
}

pub fn use_it(mut user: User, value: u8) {
    let shared: &&User = &&user;
    let _profile = shared.pro$0file; // ty("field through double reference")
    let _label = shared.la$0bel(); // ty("method through double reference")
    let _deref_shared = (*(&user))$0; // ty("explicit shared deref")
    let _deref_mut = (*(&mut user))$0; // ty("explicit mutable deref")
    let _not_ref = (*value)$0; // ty("explicit non-reference deref")
}
"#,
        expect![[r#"
            nominal struct analysis_reference_autoderef[lib]::crate::Profile // field through double reference
            nominal struct analysis_reference_autoderef[lib]::crate::Label // method through double reference
            nominal struct analysis_reference_autoderef[lib]::crate::User // explicit shared deref
            nominal struct analysis_reference_autoderef[lib]::crate::User // explicit mutable deref
            <unknown> // explicit non-reference deref
        "#]],
    );
}

#[test]
fn selects_methods_by_the_adjusted_self_parameter() {
    check_analysis(
        r#"
//- /src/lib.rs crate:method_receiver_selection
pub trait Record {
    type Output;
    fn record(&self) -> Self::Output;
}
pub struct Predicate;
impl Record for Predicate {
    type Output = u8;
    fn record(&self) -> u8 { 0 }
}
impl<T> Record for &T {
    type Output = u16;
    fn record(&self) -> u16 { 0 }
}
pub enum Gate { Direct(Predicate) }
pub struct ReferenceOnly;

#[lang = "deref"]
pub trait Deref {
    #[lang = "deref_target"]
    type Target;
    fn deref(&self) -> &Self::Target;
}
pub struct Wrapper;
impl Deref for Wrapper {
    type Target = Predicate;
    fn deref(&self) -> &Predicate { missing() }
}
impl Record for Wrapper {
    type Output = u32;
    fn record(&self) -> u32 { 0 }
}
pub struct DerefOnly;
impl Deref for DerefOnly {
    type Target = Predicate;
    fn deref(&self) -> &Predicate { missing() }
}

pub struct Precedence;
impl Precedence {
    pub fn record(&mut self) -> u64 { 0 }
}
impl Record for Precedence {
    type Output = u32;
    fn record(&self) -> u32 { 0 }
}
pub trait Consume { fn record(self) -> u64; }
pub struct ValueFirst;
impl ValueFirst {
    pub fn record(&self) -> u32 { 0 }
}
impl Consume for ValueFirst {
    fn record(self) -> u64 { 0 }
}
pub struct BorrowedValue;
impl Consume for &BorrowedValue {
    fn record(self) -> u64 { 0 }
}
pub struct SameReceiver;
impl SameReceiver {
    pub fn record(&self) -> u64 { 0 }
}
impl Record for SameReceiver {
    type Output = u32;
    fn record(&self) -> u32 { 0 }
}

pub fn match_binding(gate: &Gate) {
    match gate {
        Gate::Direct(predicate) => { let res$0ult = predicate.record(); } // ty("borrowed match binding")
    }
}
pub fn double_reference(predicate: &&Predicate) { let res$0ult = predicate.record(); } // ty("double reference")
pub fn reference_only(value: &ReferenceOnly) { let res$0ult = value.record(); } // ty("reference-only implementation")
pub fn own_wrapper(value: Wrapper) { let res$0ult = value.record(); } // ty("wrapper implementation before Deref")
pub fn through_deref(value: DerefOnly) { let res$0ult = value.record(); } // ty("implementation through Deref")
pub fn borrowed_before_mutable(mut value: Precedence) { let res$0ult = value.record(); } // ty("shared trait receiver before mutable inherent receiver")
pub fn mutable_receiver(value: &mut Precedence) { let res$0ult = value.record(); } // ty("mutable inherent receiver matches directly")
pub fn value_before_borrow(value: ValueFirst) { let res$0ult = value.record(); } // ty("value trait receiver before borrowed inherent receiver")
pub fn reference_by_value(value: &BorrowedValue) { let res$0ult = value.record(); } // ty("reference taken by value")
pub fn inherent_at_same_receiver(value: SameReceiver) { let res$0ult = value.record(); } // ty("inherent precedence at the same receiver")
pub fn explicit_reference(value: &&Predicate) {
    let res$0ult = <&Predicate as Record>::record(value); // ty("explicit reference Self")
}
"#,
        expect![[r#"
            u8 // borrowed match binding
            u16 // double reference
            u16 // reference-only implementation
            u32 // wrapper implementation before Deref
            u8 // implementation through Deref
            u32 // shared trait receiver before mutable inherent receiver
            u64 // mutable inherent receiver matches directly
            u64 // value trait receiver before borrowed inherent receiver
            u64 // reference taken by value
            u64 // inherent precedence at the same receiver
            u16 // explicit reference Self
        "#]],
    );
}

#[test]
fn autoderefs_core_deref_for_member_lookup_and_explicit_deref() {
    check_analysis_with_fake_sysroot(
        r#"
//- /app/src/lib.rs crate:app root:app
use std::cell::RefCell;
use std::sync::Arc;

pub struct Id;
pub struct Label;

pub struct User {
    pub id: Id,
}

impl User {
    pub fn label(&self) -> Label {
        missing()
    }
}

pub fn use_it(user: Arc<User>, tuple: Arc<(Id, Label)>, cell: RefCell<User>) {
    let _id = user.i$0d; // ty("field through Deref").in_lib("app")
    let _label = user.la$0bel(); // ty("method through Deref").in_lib("app")
    let _explicit = (*user)$0; // ty("explicit Deref").in_lib("app")
    let _tuple_field = tuple.$0$0; // ty("tuple field through Deref").in_lib("app")

    let borrowed = cell.borrow();
    let _borrowed_id = borrowed.i$0d; // ty("field through Ref Deref").in_lib("app")
    let _borrowed_label = borrowed.la$0bel(); // ty("method through Ref Deref").in_lib("app")
}
"#,
        expect![[r#"
            nominal struct app[lib]::crate::Id // field through Deref
            nominal struct app[lib]::crate::Label // method through Deref
            nominal struct app[lib]::crate::User // explicit Deref
            nominal struct app[lib]::crate::Id // tuple field through Deref
            nominal struct app[lib]::crate::Id // field through Ref Deref
            nominal struct app[lib]::crate::Label // method through Ref Deref
        "#]],
    );
}

#[test]
fn identifies_deref_by_language_item_instead_of_path_or_name() {
    check_analysis(
        r#"
//- /runtime/src/lib.rs crate:fake_runtime root:runtime
pub mod magic {
    #[lang = "deref"]
    pub trait Project {
        #[lang = "deref_target"]
        type Projected;
    }
}

//- /app/src/lib.rs crate:app root:app deps:runtime=fake_runtime
mod core {
    pub mod ops {
        pub trait Deref {
            type Target;
        }
    }
}

pub struct Id;

pub struct User {
    pub id: Id,
}

pub struct LanguageWrapper<T> {
    inner: T,
}

impl<T> runtime::magic::Project for LanguageWrapper<T> {
    type Projected = T;
}

pub struct NameOnlyWrapper<T> {
    inner: T,
}

impl<T> core::ops::Deref for NameOnlyWrapper<T> {
    type Target = T;
}

pub fn use_it(language: LanguageWrapper<User>, name_only: NameOnlyWrapper<User>) {
    let _language_id = language.i$0d; // ty("Deref follows language identity").in_lib("app")
    let _name_only_id = name_only.i$0d; // ty("Deref ignores path and name alone").in_lib("app")
}
"#,
        expect![[r#"
            nominal struct app[lib]::crate::Id // Deref follows language identity
            <unknown> // Deref ignores path and name alone
        "#]],
    );
}

#[test]
fn rejects_uncertain_nested_generic_deref_impls_for_member_lookup() {
    check_analysis_with_fake_sysroot(
        r#"
//- /app/src/lib.rs crate:app root:app
pub struct Id;
pub struct Foo;
pub struct Option<T> {
    value: T,
}
pub struct Result<T> {
    value: T,
}

pub struct User {
    pub id: Id,
}

pub struct Wrapper<T> {
    inner: T,
}

impl<T> core::ops::Deref for Wrapper<Option<T>> {
    type Target = User;
}

pub fn use_it(wrapper: Wrapper<Result<Foo>>) {
    let _id = wrapper.i$0d; // ty("rejected nested Deref impl").in_lib("app")
}
"#,
        expect![[r#"
            <unknown> // rejected nested Deref impl
        "#]],
    );
}

#[test]
fn rejects_defaulted_deref_impl_params_with_unproven_bounds() {
    check_analysis_with_fake_sysroot(
        r#"
//- /app/src/lib.rs crate:app root:app
pub trait Allocator {}

pub struct Global;
pub struct Id;

pub struct User {
    pub id: Id,
}

pub struct Wrapper<T = Global> {
    inner: T,
}

impl<T: Allocator> core::ops::Deref for Wrapper<T> {
    type Target = User;
}

pub fn use_it(wrapper: Wrapper) {
    let _id = wrapper.i$0d; // ty("unproven defaulted Deref bound").in_lib("app")
}
"#,
        expect![[r#"
            <unknown> // unproven defaulted Deref bound
        "#]],
    );
}

#[test]
fn aggregates_same_depth_deref_targets_before_resolving_members() {
    check_analysis_with_fake_sysroot(
        r#"
//- /app/src/lib.rs crate:app root:app
pub struct UserId;
pub struct ProjectId;
pub struct UserLabel;
pub struct ProjectLabel;

pub struct User {
    pub id: UserId,
}

impl User {
    pub fn label(&self) -> UserLabel {
        missing()
    }
}

pub struct Project {
    pub id: ProjectId,
}

impl Project {
    pub fn label(&self) -> ProjectLabel {
        missing()
    }
}

pub struct Wrapper;

impl core::ops::Deref for Wrapper {
    type Target = User;
}

impl core::ops::Deref for Wrapper {
    type Target = Project;
}

pub fn use_it(wrapper: Wrapper) {
    let _id = wrapper.i$0d; // ty("ambiguous same-depth Deref field").in_lib("app")
    let _label = wrapper.la$0bel(); // ty("ambiguous same-depth Deref method").in_lib("app")
}
"#,
        expect![[r#"
            <unknown> // ambiguous same-depth Deref field
            <unknown> // ambiguous same-depth Deref method
        "#]],
    );
}

#[test]
fn alternates_reference_and_trait_deref_for_member_lookup() {
    check_analysis_with_fake_sysroot(
        r#"
//- /app/src/lib.rs crate:app root:app
pub struct Id;
pub struct Label;

pub struct User {
    pub id: Id,
}

impl User {
    pub fn label(&self) -> Label {
        missing()
    }
}

pub struct Box<T> {
    inner: T,
}

impl<T> std::ops::Deref for Box<T> {
    type Target = T;
}

pub fn use_it(user_box: &Box<User>, ref_box: &Box<&User>) {
    let _id = user_box.i$0d; // ty("field through &Box<User>").in_lib("app")
    let _label = (&*user_box).la$0bel(); // ty("method through &*&Box<User>").in_lib("app")
    let _nested_id = (&*ref_box).i$0d; // ty("field through &*&Box<&User>").in_lib("app")
}
"#,
        expect![[r#"
            nominal struct app[lib]::crate::Id // field through &Box<User>
            nominal struct app[lib]::crate::Label // method through &*&Box<User>
            nominal struct app[lib]::crate::Id // field through &*&Box<&User>
        "#]],
    );
}

#[test]
fn returns_primitive_type_paths() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_primitive_type_at
pub struct Slot<T> {
    pub value: T,
}

pub fn use_it() {
    let count$0: u8 = 0; // ty("type at primitive binding")
    let text$0: &str = todo!(); // ty("type at primitive reference binding")

    let slot: Slot<bool> = todo!();
    let flag = slot.va$0lue; // ty("type at propagated primitive generic")
}
"#,
        expect![[r#"
            u8 // type at primitive binding
            &str // type at primitive reference binding
            bool // type at propagated primitive generic
        "#]],
    );
}

#[test]
fn returns_structural_tuple_array_and_slice_types() {
    check_analysis_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:analysis_structural_type_at
const N: usize = 3;

pub struct Holder;

impl Holder {
    const N: usize = 4;

    pub fn use_self(value: u8) {
        let self_repeat = [value; Self::N]$0; // ty("self repeat array expression").in_lib("analysis_structural_type_at")
    }
}

pub fn use_it(pair: (u8, bool), array: [u8; 3], slice: &[u8], value: u8) {
    let annotated_tuple$0: (u8, bool) = pair; // ty("annotated tuple binding").in_lib("analysis_structural_type_at")
    let annotated_array$0: [u8; 3] = array; // ty("annotated array binding").in_lib("analysis_structural_type_at")
    let annotated_slice$0: &[u8] = slice; // ty("annotated slice binding").in_lib("analysis_structural_type_at")
    let tuple_expr = (value, true)$0; // ty("tuple expression").in_lib("analysis_structural_type_at")
    let array_expr = [value, value]$0; // ty("array expression").in_lib("analysis_structural_type_at")
    let repeat_expr = [value; 3]$0; // ty("repeat array expression").in_lib("analysis_structural_type_at")
    let named_repeat = [value; N]$0; // ty("named repeat array expression").in_lib("analysis_structural_type_at")
    let spaced_repeat = [value; "a  b".len()
        + 1]$0; // ty("spaced repeat array expression").in_lib("analysis_structural_type_at")
    let tuple_field = pair.$0$0; // ty("tuple field").in_lib("analysis_structural_type_at")
    let indexed = array[0]$0; // ty("array index").in_lib("analysis_structural_type_at")
    let (left, right) = tuple_expr;
    let _left = le$0ft; // ty("tuple pattern left").in_lib("analysis_structural_type_at")
    let _right = ri$0ght; // ty("tuple pattern right").in_lib("analysis_structural_type_at")
    let [first, ..] = array_expr;
    let _first = fir$0st; // ty("slice pattern first").in_lib("analysis_structural_type_at")
}
"#,
        expect![[r#"
            [u8; _] // self repeat array expression
            (u8, bool) // annotated tuple binding
            [u8; 3] // annotated array binding
            &[u8] // annotated slice binding
            (u8, bool) // tuple expression
            [u8; 2] // array expression
            [u8; 3] // repeat array expression
            [u8; _] // named repeat array expression
            [u8; _] // spaced repeat array expression
            u8 // tuple field
            u8 // array index
            u8 // tuple pattern left
            bool // tuple pattern right
            u8 // slice pattern first
        "#]],
    );
}

#[test]
fn returns_index_types_through_references() {
    check_analysis_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:analysis_ref_index_type_at
pub fn use_it(slice: &[u8], array_ref: &[bool; 3], nested_slice: &&[u16]) {
    let slice_item = slice[0]$0; // ty("slice reference index").in_lib("analysis_ref_index_type_at")
    let array_item = array_ref[0]$0; // ty("array reference index").in_lib("analysis_ref_index_type_at")
    let nested_item = nested_slice[0]$0; // ty("nested slice reference index").in_lib("analysis_ref_index_type_at")
}
"#,
        expect![[r#"
            u8 // slice reference index
            bool // array reference index
            u16 // nested slice reference index
        "#]],
    );
}

#[test]
fn resolves_structural_slice_inherent_method_types() {
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
    let first = packages.first$0_ref(); // ty("slice generic method return").in_lib("app")
    let count = packages.le$0n(); // ty("slice len method return").in_lib("app")
    let array_first = array.first$0_ref(); // ty("array generic method return").in_lib("app")
    let array_count = array.le$0n(); // ty("array len method return").in_lib("app")
    let array_ref_count = array_ref.le$0n(); // ty("array ref len method return").in_lib("app")
}
"#,
        expect![[r#"
            &nominal struct app[lib]::crate::Package // slice generic method return
            usize // slice len method return
            &nominal struct app[lib]::crate::Package // array generic method return
            usize // array len method return
            usize // array ref len method return
        "#]],
    );
}

#[test]
fn resolves_trait_method_types_for_unkeyed_and_blanket_impl_receivers() {
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
    let scalar_label = scalar.scalar_$0label(); // ty("primitive trait method return").in_lib("app")
    let array_element = array.array_$0element(); // ty("array trait associated return").in_lib("app")
    let blanket_label = user.blanket_$0label(); // ty("blanket trait method return").in_lib("app")
    let same_reference = reference.reference_$0identity(); // ty("reference trait method return").in_lib("app")
}
"#,
        expect![[r#"
            nominal struct runtime[lib]::crate::Label // primitive trait method return
            &nominal struct app[lib]::crate::User // array trait associated return
            nominal struct runtime[lib]::crate::Label // blanket trait method return
            &nominal struct app[lib]::crate::User // reference trait method return
        "#]],
    );
}

#[test]
fn resolves_fake_sysroot_array_into_iter_method_return_type() {
    check_analysis_with_fake_sysroot(
        r#"
//- /app/src/lib.rs crate:app root:app
pub struct Package;

pub fn use_it(array: [Package; 3]) {
    let iterator = array.into_$0iter(); // ty("array into_iter return").in_lib("app")
}
"#,
        expect![[r#"
            nominal struct core[lib]::crate::array::IntoIter<nominal struct app[lib]::crate::Package, 3> // array into_iter return
        "#]],
    );
}

#[test]
fn builtin_macro_types_ignore_local_core_module() {
    check_analysis_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:app
mod core {}

pub fn use_it() {
    let args$0 = format_args!("hello"); // ty("format arguments").in_lib("app")
    let maybe_env$0 = option_env!("HOME"); // ty("optional environment variable").in_lib("app")
}
"#,
        expect![[r#"
            nominal struct core[lib]::crate::fmt::Arguments // format arguments
            nominal enum core[lib]::crate::option::Option<&str> // optional environment variable
        "#]],
    );
}

#[test]
fn resolves_source_backed_builtin_associated_macro_items() {
    check_analysis_with_fake_sysroot(
        r#"
//- /app/src/lib.rs crate:app root:app
pub struct Selected;
pub struct Included;
pub struct Value;

#[rustc_builtin_macro]
macro_rules! include {
    ($($args:tt)*) => {};
}

impl Value {
    core::cfg_select! {
        true => {
            pub fn selected(&self) -> Selected {
                missing()
            }
        }
    }

    include!("included.rs");
}

pub fn use_it(value: Value) {
    let selected = value.$0selected(); // ty("cfg-selected associated method return").in_lib("app")
    let included = value.$0included(); // ty("included associated method return").in_lib("app")
}

//- /app/src/included.rs
pub fn included(&self) -> Included {
    missing()
}
"#,
        expect![[r#"
            nominal struct app[lib]::crate::Selected // cfg-selected associated method return
            nominal struct app[lib]::crate::Included // included associated method return
        "#]],
    );
}

#[test]
fn propagates_for_loop_item_types_from_into_iterator() {
    check_analysis_with_fake_sysroot(
        r#"
//- /app/src/lib.rs crate:app root:app
pub struct Package;
pub struct UserId;
pub struct Event;

pub struct Bag<T> {
    value: T,
}

pub struct BagIter<T> {
    value: T,
}

impl<T> core::iter::Iterator for BagIter<T> {
    type Item = T;
}

impl<T> core::iter::IntoIterator for Bag<T> {
    type Item = T;
    type IntoIter = BagIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        missing()
    }
}

pub struct Events;

impl core::iter::Iterator for Events {
    type Item = Event;
}

pub struct KeyStream<T> {
    value: T,
}

impl<T> core::iter::Iterator for KeyStream<T> {
    type Item = T;
}

pub struct KeyMap<T> {
    value: T,
}

impl<T> KeyMap<T> {
    pub fn concrete_keys(&self) -> KeyStream<T> {
        missing()
    }

    pub fn opaque_keys(&self) -> impl core::iter::Iterator<Item = T> {
        missing()
    }
}

pub fn use_it(
    packages: &[Package],
    array: [Package; 3],
    pairs: [(Package, UserId); 2],
    bag: Bag<UserId>,
    events: Events,
    key_map: KeyMap<UserId>,
) {
    for borrowed in packages {
        let _borrowed = borr$0owed; // ty("for item from borrowed slice").in_lib("app")
    }

    for owned in array {
        let _owned = ow$0ned; // ty("for item from array").in_lib("app")
    }

    for (package, user_id) in pairs {
        let _package = pack$0age; // ty("for tuple item first field").in_lib("app")
        let _user_id = user_$0id; // ty("for tuple item second field").in_lib("app")
    }

    for user_id in bag {
        let _bag_user_id = user_$0id; // ty("for item from nominal impl").in_lib("app")
    }

    for event in events {
        let _event = eve$0nt; // ty("for item from iterator blanket impl").in_lib("app")
    }

    let _opaque_keys = key_map.opaque_keys()$0; // ty("opaque iterator return").in_lib("app")

    for concrete_key in key_map.concrete_keys() {
        let _concrete_key = concrete_$0key; // ty("for item from concrete iterator return").in_lib("app")
    }

    for opaque_key in key_map.opaque_keys() {
        let _opaque_key = opaque_$0key; // ty("for item from opaque iterator return").in_lib("app")
    }
}
"#,
        expect![[r#"
            &nominal struct app[lib]::crate::Package // for item from borrowed slice
            nominal struct app[lib]::crate::Package // for item from array
            nominal struct app[lib]::crate::Package // for tuple item first field
            nominal struct app[lib]::crate::UserId // for tuple item second field
            nominal struct app[lib]::crate::UserId // for item from nominal impl
            nominal struct app[lib]::crate::Event // for item from iterator blanket impl
            impl trait core[lib]::crate::iter::Iterator<Item = nominal struct app[lib]::crate::UserId> // opaque iterator return
            nominal struct app[lib]::crate::UserId // for item from concrete iterator return
            nominal struct app[lib]::crate::UserId // for item from opaque iterator return
        "#]],
    );
}

#[test]
fn propagates_for_loop_item_types_from_method_returned_slice() {
    check_analysis_with_fake_sysroot(
        r#"
//- /storage/src/lib.rs crate:storage root:storage
pub struct ImportData;

pub struct DefMap;

impl DefMap {
    pub fn imports(&self) -> &[ImportData] {
        missing()
    }
}

pub struct DefMapBuilder {
    incomplete: DefMap,
}

impl DefMapBuilder {
    pub fn partial(&self) -> PartialDefMap<'_> {
        PartialDefMap {
            def_map: &self.incomplete,
        }
    }
}

pub struct PartialDefMap<'a> {
    def_map: &'a DefMap,
}

impl<'a> PartialDefMap<'a> {
    pub fn imports(&self) -> &'a [ImportData] {
        self.def_map.imports()
    }
}

//- /app/src/lib.rs crate:app root:app deps:storage
use storage::{DefMap, DefMapBuilder};

pub struct BuildState {
    builder: DefMapBuilder,
}

pub fn use_it(def_map: &DefMap, state: &BuildState) {
    for import in def_map.imports() {
        let _import = imp$0ort; // ty("for item from method returned slice").in_lib("app")
    }

    for chained in state.builder.partial().imports() {
        let _chained = chai$0ned; // ty("for item from chained method returned slice").in_lib("app")
    }
}
"#,
        expect![[r#"
            &nominal struct storage[lib]::crate::ImportData // for item from method returned slice
            &nominal struct storage[lib]::crate::ImportData // for item from chained method returned slice
        "#]],
    );
}

#[test]
fn fake_sysroot_iterator_chain_infers_map_and_collect_types() {
    check_analysis_with_fake_sysroot(
        r#"
//- /app/src/lib.rs crate:app root:app
pub struct Field;
pub struct Rendered;

pub fn missing<T>() -> T {}

pub fn bar(_id: usize, _field: &Field) -> Rendered {
    missing()
}

pub fn use_it(fields: &[Field]) {
    let values = fields
        .iter()
        .enumerate()
        .map(|(id, f)| bar(id, f$0)) // ty("map closure field").in_lib("app")
        .collect::<Vec<_>>();

    let _values = val$0ues; // ty("collected mapped values").in_lib("app")
}
"#,
        expect![[r#"
            &nominal struct app[lib]::crate::Field // map closure field
            nominal struct alloc[lib]::crate::vec::Vec<nominal struct app[lib]::crate::Rendered, nominal struct alloc[lib]::crate::vec::Global> // collected mapped values
        "#]],
    );
}

#[test]
fn propagates_for_loop_item_types_from_slice_iter_method() {
    check_analysis_with_fake_sysroot(
        r#"
//- /storage/src/lib.rs crate:storage root:storage
pub struct ImportData;

pub struct DefMap;

impl DefMap {
    pub fn imports(&self) -> &[ImportData] {
        missing()
    }
}

//- /app/src/lib.rs crate:app root:app deps:storage
use storage::DefMap;

pub fn use_it(def_map: &DefMap) {
    let iter = def_map.imports().it$0er(); // ty("slice iter method return").in_lib("app")

    for imp$0ort in def_map.imports().iter() { // hover("hover for item from slice iter method").in_lib("app")
        let _import = imp$0ort; // ty("for item from slice iter method").in_lib("app")
    }
}
"#,
        expect![[r#"
            nominal struct core[lib]::crate::slice::Iter<'_, nominal struct storage[lib]::crate::ImportData> // slice iter method return

            // hover for item from slice iter method
            range: 6:9-6:15
            block:
              kind: variable
              signature:
                let import: &ImportData

            &nominal struct storage[lib]::crate::ImportData // for item from slice iter method
        "#]],
    );
}

#[test]
fn propagates_for_loop_items_from_project_style_import_helpers() {
    check_analysis_with_fake_sysroot(
        r#"
//- /storage/src/lib.rs crate:storage root:storage
use core::iter;

pub struct ImportId;
pub struct ImportData;

pub struct DefMap;

impl DefMap {
    pub fn imports(&self) -> &[ImportData] {
        missing()
    }

    pub fn imports_with_ids(&self) -> impl iter::Iterator<Item = (ImportId, &ImportData)> {
        missing()
    }
}

pub struct DefMapBuilder {
    pub incomplete: DefMap,
}

impl DefMapBuilder {
    pub fn partial(&self) -> PartialDefMap<'_> {
        PartialDefMap {
            def_map: &self.incomplete,
        }
    }
}

pub struct PartialDefMap<'a> {
    def_map: &'a DefMap,
}

impl<'a> PartialDefMap<'a> {
    pub fn imports(&self) -> &'a [ImportData] {
        self.def_map.imports()
    }

    pub fn imports_with_ids(&self) -> impl iter::Iterator<Item = (ImportId, &'a ImportData)> + 'a {
        self.def_map.imports_with_ids()
    }
}

pub struct CrateState {
    pub def_map_builder: DefMapBuilder,
}

//- /app/src/lib.rs crate:app root:app deps:storage
use storage::CrateState;

pub fn use_it(state: &CrateState) {
    for imp$0ort in state.def_map_builder.partial().imports().iter() { // hover("hover item from project-style slice iter").in_lib("app")
        let _import = imp$0ort; // ty("use item from project-style slice iter").in_lib("app")
    }

    for (import_$0id, imp$1ort) in state // hover("hover import id from project-style opaque iterator").in_lib("app"), hover("hover item from project-style opaque iterator").in_lib("app")
        .def_map_builder
        .partial()
        .imports_with_ids()
    {
        let _import_id = import$0_id; // ty("use import id from project-style opaque iterator").in_lib("app")
        let _import = imp$0ort; // ty("use item from project-style opaque iterator").in_lib("app")
    }
}
"#,
        expect![[r#"
            // hover item from project-style slice iter
            range: 4:9-4:15
            block:
              kind: variable
              signature:
                let import: &ImportData

            &nominal struct storage[lib]::crate::ImportData // use item from project-style slice iter

            // hover import id from project-style opaque iterator
            range: 8:10-8:19
            block:
              kind: variable
              signature:
                let import_id: ImportId

            // hover item from project-style opaque iterator
            range: 8:21-8:27
            block:
              kind: variable
              signature:
                let import: &ImportData

            nominal struct storage[lib]::crate::ImportId // use import id from project-style opaque iterator
            &nominal struct storage[lib]::crate::ImportData // use item from project-style opaque iterator
        "#]],
    );
}

#[test]
fn primitive_type_paths_respect_type_namespace_shadowing() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_primitive_shadow_type_at
pub struct usize(pub u8);

pub struct Holder {
    pub value: us$0ize, // ty("type at shadowed signature path")
}

pub fn module_shadow() {
    let value$0: usize = usize(1); // ty("type at shadowed module binding")
}

pub fn local_shadow() {
    struct u8;
    let value$0: u8 = u8; // ty("type at shadowed local binding")
}
"#,
        expect![[r#"
            nominal struct analysis_primitive_shadow_type_at[lib]::crate::usize // type at shadowed signature path
            nominal struct analysis_primitive_shadow_type_at[lib]::crate::usize // type at shadowed module binding
            nominal struct fn analysis_primitive_shadow_type_at[lib]::crate::local_shadow::u8 // type at shadowed local binding
        "#]],
    );
}

#[test]
fn returns_associated_function_and_enum_variant_call_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_associated_path_type
pub struct Widget;

impl Widget {
    pub fn create() -> Self {
        Widget
    }
}

pub enum Action {
    Configure(Widget),
}

pub fn use_it() {
    let widget = Widget::create($0); // ty("type at associated function call")
    let action = Action::Configure(widget)$0; // ty("type at enum variant call")
}
"#,
        expect![[r#"
            nominal struct analysis_associated_path_type[lib]::crate::Widget // type at associated function call
            nominal enum analysis_associated_path_type[lib]::crate::Action // type at enum variant call
        "#]],
    );
}

#[test]
fn returns_bin_root_dependency_types() {
    check_analysis(
        r#"
//- /crates/dep/src/lib.rs crate:dep root:crates/dep
pub struct Thing;

//- /crates/app/src/lib.rs crate:app root:crates/app deps:dep
pub struct Api;

//- /crates/app/src/main.rs crate:app root:crates/app deps:dep kind:bin name:app-bin
fn main() {
    let thing$0: dep::Thing = todo!(); // ty("type at bin dependency binding").in_bin("app")
}
"#,
        expect![[r#"
            nominal struct dep[lib]::crate::Thing // type at bin dependency binding
        "#]],
    );
}

#[test]
fn returns_field_access_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_field_type
pub struct Profile;

pub struct User {
    pub profile: Profile,
}

pub fn use_it(user: User) {
    let _typed: Profile = user.pro$0file; // ty("type at field")
}
"#,
        expect![[r#"
            nominal struct analysis_field_type[lib]::crate::Profile // type at field
        "#]],
    );
}

#[test]
fn propagates_basic_generic_arguments_through_fields_and_methods() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_generic_type_at
pub struct User;
pub struct Error;

pub struct Option<T> {
    pub value: T,
}

pub struct Vec<T> {
    pub first: T,
}

pub struct Result<T, E> {
    pub ok: T,
    pub err: E,
}

pub struct Wrapper<T> {
    pub value: T,
}

impl<U> Wrapper<U> {
    pub fn get(&self) -> U {
        missing()
    }
}

pub fn use_it() {
    let wrapped: Wrapper<Result<Vec<Option<User>>, Error>>;
    let _result = wrapped.va$0lue; // ty("generic result field")
    let _vec = wrapped.value.o$0k; // ty("generic vec field")
    let _option = wrapped.value.ok.f$0irst; // ty("generic option field")
    let _user = wrapped.value.ok.first.va$0lue; // ty("generic user field")
    let _method = wrapped.ge$0t(); // ty("generic method return")
}
"#,
        expect![[r#"
            nominal struct analysis_generic_type_at[lib]::crate::Result<nominal struct analysis_generic_type_at[lib]::crate::Vec<nominal struct analysis_generic_type_at[lib]::crate::Option<nominal struct analysis_generic_type_at[lib]::crate::User>>, nominal struct analysis_generic_type_at[lib]::crate::Error> // generic result field
            nominal struct analysis_generic_type_at[lib]::crate::Vec<nominal struct analysis_generic_type_at[lib]::crate::Option<nominal struct analysis_generic_type_at[lib]::crate::User>> // generic vec field
            nominal struct analysis_generic_type_at[lib]::crate::Option<nominal struct analysis_generic_type_at[lib]::crate::User> // generic option field
            nominal struct analysis_generic_type_at[lib]::crate::User // generic user field
            nominal struct analysis_generic_type_at[lib]::crate::Result<nominal struct analysis_generic_type_at[lib]::crate::Vec<nominal struct analysis_generic_type_at[lib]::crate::Option<nominal struct analysis_generic_type_at[lib]::crate::User>>, nominal struct analysis_generic_type_at[lib]::crate::Error> // generic method return
        "#]],
    );
}

#[test]
fn applies_explicit_generic_call_arguments_to_return_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_explicit_generic_call_args
fn missing<T>() -> T {
    loop {}
}

pub struct User;
pub struct Project;

pub fn make<T>() -> T {
    missing()
}

pub struct Builder;

impl Builder {
    pub fn build<T>() -> T {
        missing()
    }

    pub fn get<T>(&self) -> T {
        missing()
    }

    pub fn pair<T, U>(&self) -> (T, U) {
        missing()
    }
}

pub fn use_it(builder: Builder) {
    struct Local;

    let free = make::<User>()$0; // ty("explicit free function return")
    let associated = Builder::build::<Project>()$0; // ty("explicit associated function return")
    let method = builder.get::<User>()$0; // ty("explicit method return")
    let pair = builder.pair::<User, Project>()$0; // ty("explicit multi-param method return")
    let local = make::<Local>()$0; // ty("explicit body-local call return")
}
"#,
        expect![[r#"
            nominal struct analysis_explicit_generic_call_args[lib]::crate::User // explicit free function return
            nominal struct analysis_explicit_generic_call_args[lib]::crate::Project // explicit associated function return
            nominal struct analysis_explicit_generic_call_args[lib]::crate::User // explicit method return
            (nominal struct analysis_explicit_generic_call_args[lib]::crate::User, nominal struct analysis_explicit_generic_call_args[lib]::crate::Project) // explicit multi-param method return
            nominal struct fn analysis_explicit_generic_call_args[lib]::crate::use_it::Local // explicit body-local call return
        "#]],
    );
}

#[test]
fn infers_basic_generic_call_arguments() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_generic_call_arg_inference
fn missing<T>() -> T {
    loop {}
}

pub struct User;
pub struct Project;

pub struct Wrapper<T> {
    value: T,
}

pub fn id<T>(value: T) -> T {
    value
}

pub fn pair<T, U>(left: T, right: U) -> (T, U) {
    missing()
}

pub fn same<T>(left: T, right: T) -> T {
    missing()
}

pub struct Builder;

impl Builder {
    pub fn wrap<T>(value: T) -> Wrapper<T> {
        missing()
    }

    pub fn echo<T>(&self, value: T) -> T {
        value
    }

    pub fn clone_ref<T>(&self, value: &T) -> T {
        missing()
    }
}

pub fn use_it(builder: Builder, user: User, project: Project) {
    let id_value = id(user)$0; // ty("inferred free function return")
    let pair_value = pair(user, project)$0; // ty("inferred multi-param return")
    let wrapped = Builder::wrap(user)$0; // ty("inferred associated function return")
    let echoed = builder.echo(project)$0; // ty("inferred method return")
    let cloned = builder.clone_ref(&user)$0; // ty("inferred reference param return")
    let conflict = same(user, project)$0; // ty("conflicting inferred params")
}
"#,
        expect![[r#"
            nominal struct analysis_generic_call_arg_inference[lib]::crate::User // inferred free function return
            (nominal struct analysis_generic_call_arg_inference[lib]::crate::User, nominal struct analysis_generic_call_arg_inference[lib]::crate::Project) // inferred multi-param return
            nominal struct analysis_generic_call_arg_inference[lib]::crate::Wrapper<nominal struct analysis_generic_call_arg_inference[lib]::crate::User> // inferred associated function return
            nominal struct analysis_generic_call_arg_inference[lib]::crate::Project // inferred method return
            nominal struct analysis_generic_call_arg_inference[lib]::crate::User // inferred reference param return
            nominal struct analysis_generic_call_arg_inference[lib]::crate::User // conflicting inferred params
        "#]],
    );
}

#[test]
fn inferred_function_generics_shadow_impl_generics() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_generic_call_arg_shadowing
fn missing<T>() -> T {
    loop {}
}

pub struct User;
pub struct Project;

pub struct Container<T> {
    value: T,
}

impl<T> Container<T> {
    pub fn current(&self) -> T {
        missing()
    }

    pub fn replace<T>(&self, value: T) -> T {
        value
    }
}

pub fn use_it(container: Container<User>, project: Project) {
    let current = container.current()$0; // ty("impl generic method return")
    let replaced = container.replace(project)$0; // ty("shadowed method generic return")
}
"#,
        expect![[r#"
            nominal struct analysis_generic_call_arg_shadowing[lib]::crate::User // impl generic method return
            nominal struct analysis_generic_call_arg_shadowing[lib]::crate::Project // shadowed method generic return
        "#]],
    );
}

#[test]
fn resolves_lifetime_parameterized_receiver_method_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_lifetime_receiver_method_type
fn missing<T>() -> T {
    loop {}
}

pub struct Module;
pub struct DebugStruct;

pub struct TargetScopeCollector<'db> {
    module: &'db Module,
}

impl<'db> TargetScopeCollector<'db> {
    fn alloc_module(&mut self) -> Module {
        missing()
    }

    fn collect_items(&mut self) -> Module {
        self.alloc$0_module() // ty("self method in lifetime impl")
    }

    fn collect(&mut self) -> Module {
        self.collect$0_items() // ty("self sibling method in lifetime impl")
    }
}

pub mod fmt {
    pub struct Formatter<'a> {
        marker: &'a (),
    }
}

impl<'a> fmt::Formatter<'a> {
    fn debug_struct(&mut self) -> crate::DebugStruct {
        crate::missing()
    }
}

pub fn use_it<'db>(
    collector: &mut TargetScopeCollector<'db>,
    formatter: &mut fmt::Formatter<'_>,
) {
    let _items = collector.collect$0_items(); // ty("external method on lifetime receiver")
    let _debug = formatter.debug$0_struct(); // ty("method on Formatter placeholder lifetime")
}
"#,
        expect![[r#"
            nominal struct analysis_lifetime_receiver_method_type[lib]::crate::Module // self method in lifetime impl
            nominal struct analysis_lifetime_receiver_method_type[lib]::crate::Module // self sibling method in lifetime impl
            nominal struct analysis_lifetime_receiver_method_type[lib]::crate::Module // external method on lifetime receiver
            nominal struct analysis_lifetime_receiver_method_type[lib]::crate::DebugStruct // method on Formatter placeholder lifetime
        "#]],
    );
}

#[test]
fn does_not_treat_concrete_impl_self_args_as_type_params() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_concrete_impl_args
pub struct User;
pub struct Error;

pub struct Wrapper<T> {
    value: T,
}

impl<T> Wrapper<T> {
    pub fn generic(&self) -> T {
        missing()
    }
}

impl Wrapper<User> {
    pub fn user_only(&self) -> User {
        missing()
    }
}

pub fn use_it(user: Wrapper<User>, error: Wrapper<Error>) {
    let _user = user.user$0_only(); // ty("concrete impl method on matching receiver")
    let _error = error.gen$0eric(); // ty("generic impl method on concrete receiver")
    let _missing = error.user$0_only(); // ty("concrete impl method on wrong receiver")
}
"#,
        expect![[r#"
            nominal struct analysis_concrete_impl_args[lib]::crate::User // concrete impl method on matching receiver
            nominal struct analysis_concrete_impl_args[lib]::crate::Error // generic impl method on concrete receiver
            <unknown> // concrete impl method on wrong receiver
        "#]],
    );
}

#[test]
fn does_not_ignore_const_generic_args_when_matching_impls() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_const_impl_args
fn missing<T>() -> T {
    loop {}
}

pub struct Label;

pub struct Foo<const N: usize>;

impl Foo<1> {
    pub fn label(&self) -> Label {
        missing()
    }
}

pub fn use_semantic(foo: Foo<2>) {
    let _label = foo.la$0bel(); // ty("const impl arg mismatch")
}

pub fn use_local() {
    struct LocalLabel;
    struct LocalFoo<const N: usize>;

    impl LocalFoo<1> {
        fn label(&self) -> LocalLabel {
            missing()
        }
    }

    let foo: LocalFoo<2> = missing();
    let _label = foo.la$0bel(); // ty("local const impl arg mismatch")
}
"#,
        expect![[r#"
            <unknown> // const impl arg mismatch
            <unknown> // local const impl arg mismatch
        "#]],
    );
}

#[test]
fn uses_proven_trait_impls_for_method_return_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_trait_applicability_type_at
fn missing<T>() -> T {
    loop {}
}

pub struct User;
pub struct Error;

pub struct Wrapper<T> {
    value: T,
}

pub trait BuildUser {
    fn build_user(&self) -> User;
}

impl<T> BuildUser for Wrapper<T> {
    fn build_user(&self) -> User {
        missing()
    }
}

pub trait UserOnlyBuild {
    fn user_only(&self) -> User;
}

impl UserOnlyBuild for Wrapper<User> {
    fn user_only(&self) -> User {
        missing()
    }
}

pub fn use_it(generic: Wrapper<Error>, concrete: Wrapper<Error>) {
    let maybe_user = generic.build_user();
    let _from_maybe = maybe_$0user; // ty("maybe trait method return")

    let wrong = concrete.user_only();
    let _from_wrong = wro$0ng; // ty("concrete trait impl mismatch")
}
"#,
        expect![[r#"
            nominal struct analysis_trait_applicability_type_at[lib]::crate::User // maybe trait method return
            <unknown> // concrete trait impl mismatch
        "#]],
    );
}

#[test]
fn propagates_trait_method_return_types_through_bindings() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_direct_trait_type_at
fn missing<T>() -> T {
    loop {}
}

pub struct User;
pub struct UserId;

pub trait Identify {
    fn id(&self) -> UserId;
}

impl Identify for User {
    fn id(&self) -> UserId {
        missing()
    }
}

pub fn use_it(user: User) {
    let id = user.id();
    let _again = i$0d; // ty("trait method return through binding")
}
"#,
        expect![[r#"
            nominal struct analysis_direct_trait_type_at[lib]::crate::UserId // trait method return through binding
        "#]],
    );
}

#[test]
fn implicit_trait_items_require_scope_but_explicit_trait_paths_do_not() {
    check_analysis(
        r#"
//- /runtime/src/lib.rs crate:runtime root:runtime
pub struct Value;
pub struct Output;

pub trait Hidden {
    const TOKEN: Output;
    fn make() -> Output;
    fn secret(&self) -> Output;
}

impl Hidden for Value {
    const TOKEN: Output = Output;
    fn make() -> Output { Output }
    fn secret(&self) -> Output { Output }
}

//- /app/src/lib.rs crate:app root:app deps:runtime
use runtime::Value;

pub fn inspect(value: Value) {
    let missing_method$0 = value.secret(); // ty("method without trait import").in_lib("app")
    let missing_const$0 = Value::TOKEN; // ty("const without trait import").in_lib("app")
    let missing_function$0 = Value::make(); // ty("function without trait import").in_lib("app")

    let qualified_const$0 = <Value as runtime::Hidden>::TOKEN; // ty("explicit qualified trait const").in_lib("app")
    let qualified_function$0 = <Value as runtime::Hidden>::make(); // ty("explicit qualified trait function").in_lib("app")

    {
        use runtime::Hidden;
        let visible_method$0 = value.secret(); // ty("method with block trait import").in_lib("app")
        let visible_const$0 = Value::TOKEN; // ty("const with block trait import").in_lib("app")
        let visible_function$0 = Value::make(); // ty("function with block trait import").in_lib("app")

        {
            struct Hidden;
            let shadowed_name$0 = value.secret(); // ty("type shadow keeps outer trait").in_lib("app")
        }
    }
}
"#,
        expect![[r#"
            <unknown> // method without trait import
            <unknown> // const without trait import
            <unknown> // function without trait import
            nominal struct runtime[lib]::crate::Output // explicit qualified trait const
            nominal struct runtime[lib]::crate::Output // explicit qualified trait function
            nominal struct runtime[lib]::crate::Output // method with block trait import
            nominal struct runtime[lib]::crate::Output // const with block trait import
            nominal struct runtime[lib]::crate::Output // function with block trait import
            nominal struct runtime[lib]::crate::Output // type shadow keeps outer trait
        "#]],
    );
}

#[test]
fn returns_body_local_field_access_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_local_field_type
pub struct GlobalId;

pub fn use_it() {
    struct User {
        local_id: GlobalId,
    }

    let user: User;
    let _id: GlobalId = user.loc$0al_id; // ty("type at body-local field")
}
"#,
        expect![[r#"
            nominal struct analysis_body_local_field_type[lib]::crate::GlobalId // type at body-local field
        "#]],
    );
}

#[test]
fn propagates_basic_generic_arguments_for_body_local_fields() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_local_generic_field_type
pub struct User;

pub fn use_it() {
    struct Slot<T> {
        value: T,
    }

    let slot: Slot<User>;
    let _user = slot.va$0lue; // ty("body-local generic field")
}
"#,
        expect![[r#"
            nominal struct analysis_body_local_generic_field_type[lib]::crate::User // body-local generic field
        "#]],
    );
}

#[test]
fn returns_body_local_method_call_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_local_method_type
pub struct GlobalId;

pub fn use_it() {
    struct User;

    impl User {
        fn id(&self) -> GlobalId {
            missing()
        }

        fn again(&self) -> Self {
            missing()
        }
    }

    let user: User;
    let _id: GlobalId = user.i$0d(); // ty("type at body-local method")
    let _again: User = user.a$0gain(); // ty("type at body-local Self method")
}
"#,
        expect![[r#"
            nominal struct analysis_body_local_method_type[lib]::crate::GlobalId // type at body-local method
            nominal struct fn analysis_body_local_method_type[lib]::crate::use_it::User // type at body-local Self method
        "#]],
    );
}

#[test]
fn returns_body_local_imported_value_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_local_import_type
pub fn use_it() {
    mod local {
        pub struct User;
        pub const VALUE: User = missing();
    }

    use local::User as LocalUser;
    use local::*;

    let _user: LocalUser;
    let _value = VAL$0UE; // ty("type at body-local imported value")
}
"#,
        expect![[r#"
            nominal struct fn analysis_body_local_import_type[lib]::crate::use_it::User // type at body-local imported value
        "#]],
    );
}

#[test]
fn returns_nested_body_local_impl_method_call_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_nested_body_local_method_type
pub struct GlobalId;

pub fn use_it() {
    struct User;

    {
        impl User {
            fn id(&self) -> GlobalId {
                missing()
            }
        }
    }

    let user: User;
    let _id: GlobalId = user.i$0d(); // ty("type at nested body-local method")
}
"#,
        expect![[r#"
            nominal struct analysis_nested_body_local_method_type[lib]::crate::GlobalId // type at nested body-local method
        "#]],
    );
}

#[test]
fn substitutes_body_local_impl_generics_in_method_returns() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_local_impl_generic_method_type
pub fn use_it() {
    struct User;
    struct Wrapper<T> {
        value: T,
    }

    impl<U> Wrapper<U> {
        fn get(&self) -> U {
            missing()
        }
    }

    let wrapper: Wrapper<User>;
    let _value: User = wrapper.ge$0t(); // ty("type at body-local generic impl method")
}
"#,
        expect![[r#"
            nominal struct fn analysis_body_local_impl_generic_method_type[lib]::crate::use_it::User // type at body-local generic impl method
        "#]],
    );
}

#[test]
fn propagates_enum_variant_payload_types_into_patterns() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_enum_pattern_type_at
pub struct User;
pub struct Profile;

pub enum Option<T> {
    Some(T),
    None,
}

pub enum Message<T> {
    User { profile: T },
    Empty,
}

pub fn use_it(maybe: Option<User>, message: Message<Profile>) {
    let Some(value) = maybe else { return; };
    let _from_let = val$0ue; // ty("type from tuple variant let pattern")

    let Message::User { profile } = message else { return; };
    let _from_record = pro$0file; // ty("type from record variant let pattern")
}

pub fn match_it(maybe: Option<User>) {
    match maybe {
        Option::Some(user) => {
            let _from_match = us$0er; // ty("type from tuple variant match pattern")
        }
        Option::None => {}
    }
}
"#,
        expect![[r#"
            nominal struct analysis_enum_pattern_type_at[lib]::crate::User // type from tuple variant let pattern
            nominal struct analysis_enum_pattern_type_at[lib]::crate::Profile // type from record variant let pattern
            nominal struct analysis_enum_pattern_type_at[lib]::crate::User // type from tuple variant match pattern
        "#]],
    );
}

#[test]
fn returns_tuple_field_access_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_tuple_field_type
pub struct Left;
pub struct Right;

pub struct Pair(pub Left, pub Right);

pub fn use_it(pair: Pair) {
    let _right: Right = pair.$0$1; // ty("type at tuple field")
}
"#,
        expect![[r#"
            nominal struct analysis_tuple_field_type[lib]::crate::Right // type at tuple field
        "#]],
    );
}

#[test]
fn returns_signature_path_and_field_declaration_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_signature_type_at
pub struct Profile;

pub struct User {
    pub pro$0file: Pro$1file, // ty("type at field declaration"), ty("type at field type path")
}

pub fn make(profile: Pro$0file) -> Pro$1file { // ty("type at parameter type"), ty("type at return type")
    profile
}
"#,
        expect![[r#"
            nominal struct analysis_signature_type_at[lib]::crate::Profile // type at field declaration
            nominal struct analysis_signature_type_at[lib]::crate::Profile // type at field type path
            nominal struct analysis_signature_type_at[lib]::crate::Profile // type at parameter type
            nominal struct analysis_signature_type_at[lib]::crate::Profile // type at return type
        "#]],
    );
}

#[test]
fn returns_self_type_in_impl_signatures() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_impl_self_signature_type
pub struct User;

impl User {
    pub fn new() -> Se$0lf { // ty("type at impl signature Self")
        User
    }
}
"#,
        expect![[r#"
            nominal struct analysis_impl_self_signature_type[lib]::crate::User // type at impl signature Self
        "#]],
    );
}

#[test]
fn returns_self_receiver_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_self_receiver_type
pub struct User;

impl User {
    pub fn owned(self) {
        let _owned = se$0lf; // ty("type at owned self")
    }

    pub fn shared(&self) {
        let _shared = se$0lf; // ty("type at shared self")
    }

    pub fn unique(&mut self) {
        let _unique = se$0lf; // ty("type at mutable self")
    }
}
"#,
        expect![[r#"
            nominal struct analysis_self_receiver_type[lib]::crate::User // type at owned self
            &nominal struct analysis_self_receiver_type[lib]::crate::User // type at shared self
            &mut nominal struct analysis_self_receiver_type[lib]::crate::User // type at mutable self
        "#]],
    );
}

#[test]
fn returns_body_local_struct_types_before_module_structs() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_local_struct_type
pub struct User;

pub fn make() {
    struct User;
    let local$0: Us$1er = User; // ty("type at local binding"), ty("type at local type path")
    let _again: User = loc$0al; // ty("type at local expr")
}

pub fn outside() {
    let outside$0: User = User; // ty("type at module binding")
}
"#,
        expect![[r#"
            nominal struct fn analysis_local_struct_type[lib]::crate::make::User // type at local binding
            nominal struct fn analysis_local_struct_type[lib]::crate::make::User // type at local type path
            nominal struct fn analysis_local_struct_type[lib]::crate::make::User // type at local expr
            nominal struct analysis_local_struct_type[lib]::crate::User // type at module binding
        "#]],
    );
}

#[test]
fn returns_body_local_enum_variant_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_local_enum_variant_type
pub struct GlobalId;

pub fn make() {
    enum Action {
        Start(GlobalId),
        Stop,
    }

    let _action = Action::Sta$0rt(GlobalId); // ty("type at local enum variant")
}
"#,
        expect![[r#"
            nominal enum fn analysis_local_enum_variant_type[lib]::crate::make::Action // type at local enum variant
        "#]],
    );
}

#[test]
fn returns_body_local_record_literal_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_local_record_literal_type
pub struct GlobalId;

pub fn make() {
    struct User {
        id: GlobalId,
    }

    let user$0 = Us$1er { id: GlobalId }; // ty("type at record binding"), ty("type at record literal")
}
"#,
        expect![[r#"
            nominal struct fn analysis_local_record_literal_type[lib]::crate::make::User // type at record binding
            nominal struct fn analysis_local_record_literal_type[lib]::crate::make::User // type at record literal
        "#]],
    );
}

#[test]
fn returns_scope_ordered_body_local_value_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_value_shadowing_type
pub struct Outer;
pub struct Inner;

pub fn make() {
    fn helper() -> Outer {
        Outer
    }
    let value = Outer;

    {
        fn value() -> Inner {
            Inner
        }
        let from_fn$0 = value(); // ty("type at inner function result")
    };

    {
        const helper: Inner = Inner;
        let from_const$0 = helper; // ty("type at inner const result")
    };
}
"#,
        expect![[r#"
            nominal struct analysis_body_value_shadowing_type[lib]::crate::Inner // type at inner function result
            nominal struct analysis_body_value_shadowing_type[lib]::crate::Inner // type at inner const result
        "#]],
    );
}

#[test]
fn returns_body_local_associated_item_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_local_assoc_type
pub struct GlobalId;

pub fn make() {
    struct User;

    impl User {
        const DEFAULT: GlobalId = GlobalId;
        type Id = GlobalId;
    }

    let default$0 = User::DEFAULT; // ty("type at associated const result")
    let typed$0: User::Id = GlobalId; // ty("type at associated type result")

    fn helper() {
        let default$0 = User::DEFAULT; // ty("type at nested associated const result")
        let typed$0: User::Id = GlobalId; // ty("type at nested associated type result")
    }
}
"#,
        expect![[r#"
            nominal struct analysis_body_local_assoc_type[lib]::crate::GlobalId // type at associated const result
            nominal struct analysis_body_local_assoc_type[lib]::crate::GlobalId // type at associated type result
            nominal struct analysis_body_local_assoc_type[lib]::crate::GlobalId // type at nested associated const result
            nominal struct analysis_body_local_assoc_type[lib]::crate::GlobalId // type at nested associated type result
        "#]],
    );
}

#[test]
fn returns_body_local_enum_pattern_payload_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_local_enum_pattern_type
pub struct User;
pub struct GlobalId;

pub fn make() {
    enum Action {
        Start(User),
        Named { id: GlobalId },
    }

    let action: Action = Action::Start(User);
    let Action::Start(user$0) = action; // ty("type at tuple payload")
    let named: Action = Action::Start(User);
    let Action::Named { id$0 } = named; // ty("type at record payload")
}
"#,
        expect![[r#"
            nominal struct analysis_body_local_enum_pattern_type[lib]::crate::User // type at tuple payload
            nominal struct analysis_body_local_enum_pattern_type[lib]::crate::GlobalId // type at record payload
        "#]],
    );
}

#[test]
fn returns_body_let_annotation_types_with_body_context() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_annotation_type
pub struct User;

impl User {
    pub fn capture(&self) {
        let _this: Se$0lf = self; // ty("type at body Self annotation")
    }
}

pub fn make() {
    struct User;
    let _: Us$0er = User; // ty("type at wildcard annotation")
    let (_left, _right): (Us$0er, Us$1er) = User; // ty("type at tuple annotation left"), ty("type at tuple annotation right")
}
"#,
        expect![[r#"
            nominal struct analysis_body_annotation_type[lib]::crate::User // type at body Self annotation
            nominal struct fn analysis_body_annotation_type[lib]::crate::make::User // type at wildcard annotation
            nominal struct fn analysis_body_annotation_type[lib]::crate::make::User // type at tuple annotation left
            nominal struct fn analysis_body_annotation_type[lib]::crate::make::User // type at tuple annotation right
        "#]],
    );
}
