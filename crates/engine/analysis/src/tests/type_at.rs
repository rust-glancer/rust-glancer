use expect_test::expect;

use super::utils::{AnalysisQuery, check_analysis_queries, check_analysis_queries_with_sysroot};

#[test]
fn returns_body_expression_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_type_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn helper() -> User {
    User
}

pub fn use_it() {
    let local: User = helper();
    let _typed: User = loc$type_at$al;
}
"#,
        &[AnalysisQuery::ty("type at local", "type_at")],
        expect![[r#"
            type at local
            - nominal struct analysis_type_at[lib]::crate::User
        "#]],
    );
}

#[test]
fn returns_scalar_literal_and_operator_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_scalar_type_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it(flag: bool, lhs: i32, rhs: i32) {
    let bool_lit = true$type_bool$;
    let char_lit = 'x'$type_char$;
    let byte_lit = b'x'$type_byte$;
    let int_default = 1$type_int$;
    let int_suffix = 1usize$type_usize$;
    let float_default = 1.0$type_float$;
    let float_suffix = 1.0f32$type_f32$;
    let string_lit = "text"$type_str$;
    let not_flag = (!flag)$type_not$;
    let neg_lhs = (-lhs)$type_neg$;
    let sum = (lhs + rhs)$type_sum$;
    let compare = (lhs < rhs)$type_compare$;
    let logic = (flag && false)$type_logic$;
    let bit = (lhs & rhs)$type_bit$;
    let shift = (lhs << 1)$type_shift$;
}
"#,
        &[
            AnalysisQuery::ty("bool literal", "type_bool"),
            AnalysisQuery::ty("char literal", "type_char"),
            AnalysisQuery::ty("byte literal", "type_byte"),
            AnalysisQuery::ty("default int literal", "type_int"),
            AnalysisQuery::ty("suffixed int literal", "type_usize"),
            AnalysisQuery::ty("default float literal", "type_float"),
            AnalysisQuery::ty("suffixed float literal", "type_f32"),
            AnalysisQuery::ty("string literal", "type_str"),
            AnalysisQuery::ty("not expression", "type_not"),
            AnalysisQuery::ty("neg expression", "type_neg"),
            AnalysisQuery::ty("sum expression", "type_sum"),
            AnalysisQuery::ty("comparison expression", "type_compare"),
            AnalysisQuery::ty("logic expression", "type_logic"),
            AnalysisQuery::ty("bit expression", "type_bit"),
            AnalysisQuery::ty("shift expression", "type_shift"),
        ],
        expect![[r#"
            bool literal
            - bool

            char literal
            - char

            byte literal
            - u8

            default int literal
            - i32

            suffixed int literal
            - usize

            default float literal
            - f64

            suffixed float literal
            - f32

            string literal
            - &str

            not expression
            - bool

            neg expression
            - i32

            sum expression
            - i32

            comparison expression
            - bool

            logic expression
            - bool

            bit expression
            - i32

            shift expression
            - i32
        "#]],
    );
}

#[test]
fn returns_types_for_references_try_and_await_wrappers() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_wrapper_type_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Result<T, E> {
    Ok(T),
    Err(E),
}

pub struct Error;
pub struct User;

pub fn load_user() -> Result<User, Error> {
    todo!()
}

pub async fn load_user_async() -> User {
    User
}

pub async fn use_it(mut user: User) -> Result<(), Error> {
    let _borrowed = (&user)$type_ref$;
    let _borrowed_mut = (&mut user)$type_mut_ref$;
    let typed_mut$type_mut_binding$: &mut User = todo!();
    let _loaded = load_user()?$type_try$;
    let _borrowed_loaded = (&load_user())?$type_try_borrowed_result$;
    let _awaited = load_user_async().await$type_await$;
    Result::Ok(())
}
"#,
        &[
            AnalysisQuery::ty("type at reference wrapper", "type_ref"),
            AnalysisQuery::ty("type at mutable reference wrapper", "type_mut_ref"),
            AnalysisQuery::ty("type at mutable reference binding", "type_mut_binding"),
            AnalysisQuery::ty("type at try wrapper", "type_try"),
            AnalysisQuery::ty("type at borrowed try wrapper", "type_try_borrowed_result"),
            AnalysisQuery::ty("type at await wrapper", "type_await"),
        ],
        expect![[r#"
            type at reference wrapper
            - &nominal struct analysis_wrapper_type_at[lib]::crate::User

            type at mutable reference wrapper
            - &mut nominal struct analysis_wrapper_type_at[lib]::crate::User

            type at mutable reference binding
            - &mut nominal struct analysis_wrapper_type_at[lib]::crate::User

            type at try wrapper
            - nominal struct analysis_wrapper_type_at[lib]::crate::User

            type at borrowed try wrapper
            - <unknown>

            type at await wrapper
            - nominal struct analysis_wrapper_type_at[lib]::crate::User
        "#]],
    );
}

#[test]
fn autoderefs_references_for_member_lookup_and_explicit_deref() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_reference_autoderef"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
    let _profile = shared.pro$type_field$file;
    let _label = shared.la$type_method$bel();
    let _deref_shared = (*(&user))$type_deref_shared$;
    let _deref_mut = (*(&mut user))$type_deref_mut$;
    let _not_ref = (*value)$type_deref_non_ref$;
}
"#,
        &[
            AnalysisQuery::ty("field through double reference", "type_field"),
            AnalysisQuery::ty("method through double reference", "type_method"),
            AnalysisQuery::ty("explicit shared deref", "type_deref_shared"),
            AnalysisQuery::ty("explicit mutable deref", "type_deref_mut"),
            AnalysisQuery::ty("explicit non-reference deref", "type_deref_non_ref"),
        ],
        expect![[r#"
            field through double reference
            - nominal struct analysis_reference_autoderef[lib]::crate::Profile

            method through double reference
            - nominal struct analysis_reference_autoderef[lib]::crate::Label

            explicit shared deref
            - nominal struct analysis_reference_autoderef[lib]::crate::User

            explicit mutable deref
            - nominal struct analysis_reference_autoderef[lib]::crate::User

            explicit non-reference deref
            - <unknown>
        "#]],
    );
}

#[test]
fn autoderefs_core_deref_for_member_lookup_and_explicit_deref() {
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
pub mod ops {
    pub trait Deref {
        type Target;
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

pub struct Wrapper<T> {
    inner: T,
}

impl<T> core::ops::Deref for Wrapper<T> {
    type Target = T;
}

pub fn use_it(wrapper: Wrapper<User>) {
    let _id = wrapper.i$type_deref_field$d;
    let _label = wrapper.la$type_deref_method$bel();
    let _explicit = (*wrapper)$type_deref_explicit$;
}
"#,
        &[
            AnalysisQuery::ty("field through Deref", "type_deref_field").in_lib("app"),
            AnalysisQuery::ty("method through Deref", "type_deref_method").in_lib("app"),
            AnalysisQuery::ty("explicit Deref", "type_deref_explicit").in_lib("app"),
        ],
        expect![[r#"
            field through Deref
            - nominal struct app[lib]::crate::Id

            method through Deref
            - nominal struct app[lib]::crate::Label

            explicit Deref
            - nominal struct app[lib]::crate::User
        "#]],
    );
}

#[test]
fn resolves_canonical_deref_through_absolute_core_path() {
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
pub mod ops {
    pub trait Deref {
        type Target;
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

pub struct Wrapper<T> {
    inner: T,
}

impl<T> ::core::ops::Deref for Wrapper<T> {
    type Target = T;
}

pub fn use_it(wrapper: Wrapper<User>) {
    let _id = wrapper.i$type_shadowed_core$d;
}
"#,
        &[
            AnalysisQuery::ty("Deref ignores local core shadow", "type_shadowed_core")
                .in_lib("app"),
        ],
        expect![[r#"
            Deref ignores local core shadow
            - nominal struct app[lib]::crate::Id
        "#]],
    );
}

#[test]
fn rejects_uncertain_nested_generic_deref_impls_for_member_lookup() {
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
pub mod ops {
    pub trait Deref {
        type Target;
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
    let _id = wrapper.i$type_rejected_deref$d;
}
"#,
        &[AnalysisQuery::ty("rejected nested Deref impl", "type_rejected_deref").in_lib("app")],
        expect![[r#"
            rejected nested Deref impl
            - <unknown>
        "#]],
    );
}

#[test]
fn aggregates_same_depth_deref_targets_before_resolving_members() {
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
pub mod ops {
    pub trait Deref {
        type Target;
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
    let _id = wrapper.i$type_field$d;
    let _label = wrapper.la$type_method$bel();
}
"#,
        &[
            AnalysisQuery::ty("ambiguous same-depth Deref field", "type_field").in_lib("app"),
            AnalysisQuery::ty("ambiguous same-depth Deref method", "type_method").in_lib("app"),
        ],
        expect![[r#"
            ambiguous same-depth Deref field
            - <unknown>

            ambiguous same-depth Deref method
            - <unknown>
        "#]],
    );
}

#[test]
fn alternates_reference_and_trait_deref_for_member_lookup() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["core", "std", "app"]
resolver = "3"

//- /core/Cargo.toml
[package]
name = "fake_core"
version = "0.1.0"
edition = "2024"

//- /core/src/lib.rs
pub mod ops {
    pub trait Deref {
        type Target;
    }
}

//- /std/Cargo.toml
[package]
name = "fake_std"
version = "0.1.0"
edition = "2024"

[dependencies]
core = { package = "fake_core", path = "../core" }

//- /std/src/lib.rs
pub use core::ops;

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
core = { package = "fake_core", path = "../core" }
std = { package = "fake_std", path = "../std" }

//- /app/src/lib.rs
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
    let _id = user_box.i$type_ref_trait_field$d;
    let _label = (&*user_box).la$type_explicit_ref_trait_method$bel();
    let _nested_id = (&*ref_box).i$type_explicit_ref_trait_ref_field$d;
}
"#,
        &[
            AnalysisQuery::ty("field through &Box<User>", "type_ref_trait_field").in_lib("app"),
            AnalysisQuery::ty(
                "method through &*&Box<User>",
                "type_explicit_ref_trait_method",
            )
            .in_lib("app"),
            AnalysisQuery::ty(
                "field through &*&Box<&User>",
                "type_explicit_ref_trait_ref_field",
            )
            .in_lib("app"),
        ],
        expect![[r#"
            field through &Box<User>
            - nominal struct app[lib]::crate::Id

            method through &*&Box<User>
            - nominal struct app[lib]::crate::Label

            field through &*&Box<&User>
            - nominal struct app[lib]::crate::Id
        "#]],
    );
}

#[test]
fn returns_binding_declaration_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_binding_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn helper() -> User {
    User
}

pub fn use_it() {
    let typed$type_decl$: User = helper();
}
"#,
        &[AnalysisQuery::ty(
            "type at declaration binding",
            "type_decl",
        )],
        expect![[r#"
            type at declaration binding
            - nominal struct analysis_binding_type[lib]::crate::User
        "#]],
    );
}

#[test]
fn returns_primitive_type_paths() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_primitive_type_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Slot<T> {
    pub value: T,
}

pub fn use_it() {
    let count$type_count$: u8 = 0;
    let text$type_text$: &str = todo!();

    let slot: Slot<bool> = todo!();
    let flag = slot.va$type_flag$lue;
}
"#,
        &[
            AnalysisQuery::ty("type at primitive binding", "type_count"),
            AnalysisQuery::ty("type at primitive reference binding", "type_text"),
            AnalysisQuery::ty("type at propagated primitive generic", "type_flag"),
        ],
        expect![[r#"
            type at primitive binding
            - u8

            type at primitive reference binding
            - &str

            type at propagated primitive generic
            - bool
        "#]],
    );
}

#[test]
fn returns_structural_tuple_array_and_slice_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_structural_type_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
const N: usize = 3;

pub struct Holder;

impl Holder {
    const N: usize = 4;

    pub fn use_self(value: u8) {
        let self_repeat = [value; Self::N]$type_self_repeat$;
    }
}

pub fn use_it(pair: (u8, bool), array: [u8; 3], slice: &[u8], value: u8) {
    let annotated_tuple$type_annotated_tuple$: (u8, bool) = pair;
    let annotated_array$type_annotated_array$: [u8; 3] = array;
    let annotated_slice$type_annotated_slice$: &[u8] = slice;
    let tuple_expr = (value, true)$type_tuple_expr$;
    let array_expr = [value, value]$type_array_expr$;
    let repeat_expr = [value; 3]$type_repeat_expr$;
    let named_repeat = [value; N]$type_named_repeat$;
    let tuple_field = pair.$type_tuple_field$0;
    let indexed = array[0]$type_indexed$;
    let (left, right) = tuple_expr;
    let _left = le$type_left$ft;
    let _right = ri$type_right$ght;
    let [first, ..] = array_expr;
    let _first = fir$type_first$st;
}
"#,
        &[
            AnalysisQuery::ty("annotated tuple binding", "type_annotated_tuple"),
            AnalysisQuery::ty("annotated array binding", "type_annotated_array"),
            AnalysisQuery::ty("annotated slice binding", "type_annotated_slice"),
            AnalysisQuery::ty("tuple expression", "type_tuple_expr"),
            AnalysisQuery::ty("array expression", "type_array_expr"),
            AnalysisQuery::ty("repeat array expression", "type_repeat_expr"),
            AnalysisQuery::ty("named repeat array expression", "type_named_repeat"),
            AnalysisQuery::ty("self repeat array expression", "type_self_repeat"),
            AnalysisQuery::ty("tuple field", "type_tuple_field"),
            AnalysisQuery::ty("array index", "type_indexed"),
            AnalysisQuery::ty("tuple pattern left", "type_left"),
            AnalysisQuery::ty("tuple pattern right", "type_right"),
            AnalysisQuery::ty("slice pattern first", "type_first"),
        ],
        expect![[r#"
            annotated tuple binding
            - (u8, bool)

            annotated array binding
            - [u8; 3]

            annotated slice binding
            - &[u8]

            tuple expression
            - (u8, bool)

            array expression
            - [u8; 2]

            repeat array expression
            - [u8; 3]

            named repeat array expression
            - [u8; N]

            self repeat array expression
            - [u8; Self::N]

            tuple field
            - u8

            array index
            - u8

            tuple pattern left
            - u8

            tuple pattern right
            - bool

            slice pattern first
            - u8
        "#]],
    );
}

#[test]
fn resolves_structural_slice_inherent_method_types() {
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
    let first = packages.first$type_first$_ref();
    let count = packages.le$type_len$n();
    let array_first = array.first$type_array_first$_ref();
    let array_count = array.le$type_array_len$n();
    let array_ref_count = array_ref.le$type_array_ref_len$n();
}
"#,
        &[
            AnalysisQuery::ty("slice generic method return", "type_first").in_lib("app"),
            AnalysisQuery::ty("slice len method return", "type_len").in_lib("app"),
            AnalysisQuery::ty("array generic method return", "type_array_first").in_lib("app"),
            AnalysisQuery::ty("array len method return", "type_array_len").in_lib("app"),
            AnalysisQuery::ty("array ref len method return", "type_array_ref_len").in_lib("app"),
        ],
        expect![[r#"
            slice generic method return
            - &nominal struct app[lib]::crate::Package

            slice len method return
            - usize

            array generic method return
            - &nominal struct app[lib]::crate::Package

            array len method return
            - usize

            array ref len method return
            - usize
        "#]],
    );
}

#[test]
fn propagates_for_loop_item_types_from_into_iterator() {
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
pub mod iter {
    pub trait IntoIterator {
        type Item;
    }

    pub trait Iterator {
        type Item;
    }
}

impl<'a, T> iter::IntoIterator for &'a [T] {
    type Item = &'a T;
}

impl<T, const N: usize> iter::IntoIterator for [T; N] {
    type Item = T;
}

impl<I: iter::Iterator> iter::IntoIterator for I {
    type Item = I::Item;
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
pub struct UserId;
pub struct Event;

pub struct Bag<T> {
    value: T,
}

impl<T> core::iter::IntoIterator for Bag<T> {
    type Item = T;
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
        let _borrowed = borr$type_borrowed$owed;
    }

    for owned in array {
        let _owned = ow$type_owned$ned;
    }

    for (package, user_id) in pairs {
        let _package = pack$type_tuple_package$age;
        let _user_id = user_$type_tuple_user_id$id;
    }

    for user_id in bag {
        let _bag_user_id = user_$type_bag_user_id$id;
    }

    for event in events {
        let _event = eve$type_event$nt;
    }

    let _opaque_keys = key_map.opaque_keys()$type_opaque_return$;

    for concrete_key in key_map.concrete_keys() {
        let _concrete_key = concrete_$type_concrete_key$key;
    }

    for opaque_key in key_map.opaque_keys() {
        let _opaque_key = opaque_$type_opaque_key$key;
    }
}
"#,
        &[
            AnalysisQuery::ty("for item from borrowed slice", "type_borrowed").in_lib("app"),
            AnalysisQuery::ty("for item from array", "type_owned").in_lib("app"),
            AnalysisQuery::ty("for tuple item first field", "type_tuple_package").in_lib("app"),
            AnalysisQuery::ty("for tuple item second field", "type_tuple_user_id").in_lib("app"),
            AnalysisQuery::ty("for item from nominal impl", "type_bag_user_id").in_lib("app"),
            AnalysisQuery::ty("for item from iterator blanket impl", "type_event").in_lib("app"),
            AnalysisQuery::ty("opaque iterator return", "type_opaque_return").in_lib("app"),
            AnalysisQuery::ty(
                "for item from concrete iterator return",
                "type_concrete_key",
            )
            .in_lib("app"),
            AnalysisQuery::ty("for item from opaque iterator return", "type_opaque_key")
                .in_lib("app"),
        ],
        expect![[r#"
            for item from borrowed slice
            - &nominal struct app[lib]::crate::Package

            for item from array
            - nominal struct app[lib]::crate::Package

            for tuple item first field
            - nominal struct app[lib]::crate::Package

            for tuple item second field
            - nominal struct app[lib]::crate::UserId

            for item from nominal impl
            - nominal struct app[lib]::crate::UserId

            for item from iterator blanket impl
            - nominal struct app[lib]::crate::Event

            opaque iterator return
            - impl trait fake_core[lib]::crate::iter::Iterator<Item = nominal struct app[lib]::crate::UserId>

            for item from concrete iterator return
            - nominal struct app[lib]::crate::UserId

            for item from opaque iterator return
            - nominal struct app[lib]::crate::UserId
        "#]],
    );
}

#[test]
fn propagates_for_loop_item_types_from_method_returned_slice() {
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
pub mod iter {
    pub trait IntoIterator {
        type Item;
    }
}

impl<'a, T> iter::IntoIterator for &'a [T] {
    type Item = &'a T;
}

//- /storage/Cargo.toml
[package]
name = "storage"
version = "0.1.0"
edition = "2024"

//- /storage/src/lib.rs
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
    pub fn as_incomplete_def_map(&self) -> &DefMap {
        &self.incomplete
    }
}

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
core = { package = "fake_core", path = "../core" }
storage = { path = "../storage" }

//- /app/src/lib.rs
use storage::{DefMap, DefMapBuilder};

pub struct BuildState {
    builder: DefMapBuilder,
}

pub fn use_it(def_map: &DefMap, state: &BuildState) {
    for import in def_map.imports() {
        let _import = imp$type_import$ort;
    }

    for chained in state.builder.as_incomplete_def_map().imports() {
        let _chained = chai$type_chained$ned;
    }
}
"#,
        &[
            AnalysisQuery::ty("for item from method returned slice", "type_import").in_lib("app"),
            AnalysisQuery::ty(
                "for item from chained method returned slice",
                "type_chained",
            )
            .in_lib("app"),
        ],
        expect![[r#"
            for item from method returned slice
            - &nominal struct storage[lib]::crate::ImportData

            for item from chained method returned slice
            - &nominal struct storage[lib]::crate::ImportData
        "#]],
    );
}

#[test]
fn propagates_for_loop_item_types_from_sysroot_slice_iterator() {
    check_analysis_queries_with_sysroot(
        r#"
//- /Cargo.toml
[workspace]
members = ["storage", "app"]
resolver = "3"

//- /storage/Cargo.toml
[package]
name = "storage"
version = "0.1.0"
edition = "2024"

//- /storage/src/lib.rs
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
    pub fn as_incomplete_def_map(&self) -> &DefMap {
        &self.incomplete
    }
}

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
storage = { path = "../storage" }

//- /app/src/lib.rs
use storage::{DefMap, DefMapBuilder};

pub struct BuildState {
    builder: DefMapBuilder,
}

pub fn use_it(def_map: &DefMap, state: &BuildState) {
    for import in def_map.imports() {
        let _import = imp$type_import$ort;
    }

    for chained in state.builder.as_incomplete_def_map().imports() {
        let _chained = chai$type_chained$ned;
    }
}

//- /sysroot/library/core/src/lib.rs
pub mod iter {
    pub trait IntoIterator {
        type Item;
        type IntoIter;
    }
}

pub mod slice {
    pub struct Iter<'a, T>(&'a T);
}

impl<'a, T> iter::IntoIterator for &'a [T] {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;
}

//- /sysroot/library/alloc/src/lib.rs
pub struct Alloc;

//- /sysroot/library/std/src/lib.rs
pub mod prelude {
    pub mod rust_2024 {}
}
"#,
        &[
            AnalysisQuery::ty("for item from sysroot method returned slice", "type_import")
                .in_lib("app"),
            AnalysisQuery::ty(
                "for item from sysroot chained method returned slice",
                "type_chained",
            )
            .in_lib("app"),
        ],
        expect![[r#"
            for item from sysroot method returned slice
            - &nominal struct storage[lib]::crate::ImportData

            for item from sysroot chained method returned slice
            - &nominal struct storage[lib]::crate::ImportData
        "#]],
    );
}

#[test]
fn primitive_type_paths_respect_type_namespace_shadowing() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_primitive_shadow_type_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct usize(pub u8);

pub struct Holder {
    pub value: us$type_signature$ize,
}

pub fn module_shadow() {
    let value$type_module_binding$: usize = usize(1);
}

pub fn local_shadow() {
    struct u8;
    let value$type_local_binding$: u8 = u8;
}
"#,
        &[
            AnalysisQuery::ty("type at shadowed signature path", "type_signature"),
            AnalysisQuery::ty("type at shadowed module binding", "type_module_binding"),
            AnalysisQuery::ty("type at shadowed local binding", "type_local_binding"),
        ],
        expect![[r#"
            type at shadowed signature path
            - nominal struct analysis_primitive_shadow_type_at[lib]::crate::usize

            type at shadowed module binding
            - nominal struct analysis_primitive_shadow_type_at[lib]::crate::usize

            type at shadowed local binding
            - nominal struct fn analysis_primitive_shadow_type_at[lib]::crate::local_shadow::u8
        "#]],
    );
}

#[test]
fn returns_associated_function_and_enum_variant_call_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_associated_path_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
    let widget = Widget::create($type_assoc_call$);
    let action = Action::Configure(widget)$type_variant_call$;
}
"#,
        &[
            AnalysisQuery::ty("type at associated function call", "type_assoc_call"),
            AnalysisQuery::ty("type at enum variant call", "type_variant_call"),
        ],
        expect![[r#"
            type at associated function call
            - Self struct analysis_associated_path_type[lib]::crate::Widget

            type at enum variant call
            - nominal enum analysis_associated_path_type[lib]::crate::Action
        "#]],
    );
}

#[test]
fn returns_bin_root_dependency_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub struct Thing;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

[lib]
path = "src/lib.rs"

[[bin]]
name = "app-bin"
path = "src/main.rs"

//- /crates/app/src/lib.rs
pub struct Api;

//- /crates/app/src/main.rs
fn main() {
    let thing$type_bin_dep$: dep::Thing = todo!();
}
"#,
        &[AnalysisQuery::ty("type at bin dependency binding", "type_bin_dep").in_bin("app")],
        expect![[r#"
            type at bin dependency binding
            - nominal struct dep[lib]::crate::Thing
        "#]],
    );
}

#[test]
fn returns_field_access_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_field_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Profile;

pub struct User {
    pub profile: Profile,
}

pub fn use_it(user: User) {
    let _typed: Profile = user.pro$type_field$file;
}
"#,
        &[AnalysisQuery::ty("type at field", "type_field")],
        expect![[r#"
            type at field
            - nominal struct analysis_field_type[lib]::crate::Profile
        "#]],
    );
}

#[test]
fn propagates_basic_generic_arguments_through_fields_and_methods() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_generic_type_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
    let _result = wrapped.va$type_result$lue;
    let _vec = wrapped.value.o$type_vec$k;
    let _option = wrapped.value.ok.f$type_option$irst;
    let _user = wrapped.value.ok.first.va$type_user$lue;
    let _method = wrapped.ge$type_method$t();
}
"#,
        &[
            AnalysisQuery::ty("generic result field", "type_result"),
            AnalysisQuery::ty("generic vec field", "type_vec"),
            AnalysisQuery::ty("generic option field", "type_option"),
            AnalysisQuery::ty("generic user field", "type_user"),
            AnalysisQuery::ty("generic method return", "type_method"),
        ],
        expect![[r#"
            generic result field
            - nominal struct analysis_generic_type_at[lib]::crate::Result<nominal struct analysis_generic_type_at[lib]::crate::Vec<nominal struct analysis_generic_type_at[lib]::crate::Option<nominal struct analysis_generic_type_at[lib]::crate::User>>, nominal struct analysis_generic_type_at[lib]::crate::Error>

            generic vec field
            - nominal struct analysis_generic_type_at[lib]::crate::Vec<nominal struct analysis_generic_type_at[lib]::crate::Option<nominal struct analysis_generic_type_at[lib]::crate::User>>

            generic option field
            - nominal struct analysis_generic_type_at[lib]::crate::Option<nominal struct analysis_generic_type_at[lib]::crate::User>

            generic user field
            - nominal struct analysis_generic_type_at[lib]::crate::User

            generic method return
            - nominal struct analysis_generic_type_at[lib]::crate::Result<nominal struct analysis_generic_type_at[lib]::crate::Vec<nominal struct analysis_generic_type_at[lib]::crate::Option<nominal struct analysis_generic_type_at[lib]::crate::User>>, nominal struct analysis_generic_type_at[lib]::crate::Error>
        "#]],
    );
}

#[test]
fn applies_explicit_generic_call_arguments_to_return_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_explicit_generic_call_args"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
    let free = make::<User>()$type_free$;
    let associated = Builder::build::<Project>()$type_associated$;
    let method = builder.get::<User>()$type_method$;
    let pair = builder.pair::<User, Project>()$type_pair$;
}
"#,
        &[
            AnalysisQuery::ty("explicit free function return", "type_free"),
            AnalysisQuery::ty("explicit associated function return", "type_associated"),
            AnalysisQuery::ty("explicit method return", "type_method"),
            AnalysisQuery::ty("explicit multi-param method return", "type_pair"),
        ],
        expect![[r#"
            explicit free function return
            - nominal struct analysis_explicit_generic_call_args[lib]::crate::User

            explicit associated function return
            - nominal struct analysis_explicit_generic_call_args[lib]::crate::Project

            explicit method return
            - nominal struct analysis_explicit_generic_call_args[lib]::crate::User

            explicit multi-param method return
            - (nominal struct analysis_explicit_generic_call_args[lib]::crate::User, nominal struct analysis_explicit_generic_call_args[lib]::crate::Project)
        "#]],
    );
}

#[test]
fn resolves_explicit_generic_call_arguments_from_body_scope() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_explicit_generic_call_body_scope"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
fn missing<T>() -> T {
    loop {}
}

pub fn make<T>() -> T {
    missing()
}

pub fn use_it() {
    struct Local;

    let local = make::<Local>()$type_local$;
}
"#,
        &[AnalysisQuery::ty(
            "explicit call arg from body scope",
            "type_local",
        )],
        expect![[r#"
            explicit call arg from body scope
            - nominal struct fn analysis_explicit_generic_call_body_scope[lib]::crate::use_it::Local
        "#]],
    );
}

#[test]
fn infers_basic_generic_call_arguments() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_generic_call_arg_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
    let id_value = id(user)$type_id$;
    let pair_value = pair(user, project)$type_pair$;
    let wrapped = Builder::wrap(user)$type_wrap$;
    let echoed = builder.echo(project)$type_method$;
    let cloned = builder.clone_ref(&user)$type_ref$;
    let conflict = same(user, project)$type_conflict$;
}
"#,
        &[
            AnalysisQuery::ty("inferred free function return", "type_id"),
            AnalysisQuery::ty("inferred multi-param return", "type_pair"),
            AnalysisQuery::ty("inferred associated function return", "type_wrap"),
            AnalysisQuery::ty("inferred method return", "type_method"),
            AnalysisQuery::ty("inferred reference param return", "type_ref"),
            AnalysisQuery::ty("conflicting inferred params", "type_conflict"),
        ],
        expect![[r#"
            inferred free function return
            - nominal struct analysis_generic_call_arg_inference[lib]::crate::User

            inferred multi-param return
            - (nominal struct analysis_generic_call_arg_inference[lib]::crate::User, nominal struct analysis_generic_call_arg_inference[lib]::crate::Project)

            inferred associated function return
            - nominal struct analysis_generic_call_arg_inference[lib]::crate::Wrapper<nominal struct analysis_generic_call_arg_inference[lib]::crate::User>

            inferred method return
            - nominal struct analysis_generic_call_arg_inference[lib]::crate::Project

            inferred reference param return
            - nominal struct analysis_generic_call_arg_inference[lib]::crate::User

            conflicting inferred params
            - <unknown>
        "#]],
    );
}

#[test]
fn inferred_function_generics_shadow_impl_generics() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_generic_call_arg_shadowing"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
    let current = container.current()$type_current$;
    let replaced = container.replace(project)$type_replaced$;
}
"#,
        &[
            AnalysisQuery::ty("impl generic method return", "type_current"),
            AnalysisQuery::ty("shadowed method generic return", "type_replaced"),
        ],
        expect![[r#"
            impl generic method return
            - nominal struct analysis_generic_call_arg_shadowing[lib]::crate::User

            shadowed method generic return
            - nominal struct analysis_generic_call_arg_shadowing[lib]::crate::Project
        "#]],
    );
}

#[test]
fn resolves_lifetime_parameterized_receiver_method_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_lifetime_receiver_method_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
        self.alloc$type_self_alloc$_module()
    }

    fn collect(&mut self) -> Module {
        self.collect$type_self_collect$_items()
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
    let _items = collector.collect$type_collect$_items();
    let _debug = formatter.debug$type_debug$_struct();
}
"#,
        &[
            AnalysisQuery::ty("self method in lifetime impl", "type_self_alloc"),
            AnalysisQuery::ty("self sibling method in lifetime impl", "type_self_collect"),
            AnalysisQuery::ty("external method on lifetime receiver", "type_collect"),
            AnalysisQuery::ty("method on Formatter placeholder lifetime", "type_debug"),
        ],
        expect![[r#"
            self method in lifetime impl
            - nominal struct analysis_lifetime_receiver_method_type[lib]::crate::Module

            self sibling method in lifetime impl
            - nominal struct analysis_lifetime_receiver_method_type[lib]::crate::Module

            external method on lifetime receiver
            - nominal struct analysis_lifetime_receiver_method_type[lib]::crate::Module

            method on Formatter placeholder lifetime
            - nominal struct analysis_lifetime_receiver_method_type[lib]::crate::DebugStruct
        "#]],
    );
}

#[test]
fn does_not_treat_concrete_impl_self_args_as_type_params() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_concrete_impl_args"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
    let _user = user.user$type_user_method$_only();
    let _error = error.gen$type_generic_method$eric();
    let _missing = error.user$type_wrong_method$_only();
}
"#,
        &[
            AnalysisQuery::ty(
                "concrete impl method on matching receiver",
                "type_user_method",
            ),
            AnalysisQuery::ty(
                "generic impl method on concrete receiver",
                "type_generic_method",
            ),
            AnalysisQuery::ty(
                "concrete impl method on wrong receiver",
                "type_wrong_method",
            ),
        ],
        expect![[r#"
            concrete impl method on matching receiver
            - nominal struct analysis_concrete_impl_args[lib]::crate::User

            generic impl method on concrete receiver
            - nominal struct analysis_concrete_impl_args[lib]::crate::Error

            concrete impl method on wrong receiver
            - <unknown>
        "#]],
    );
}

#[test]
fn does_not_ignore_const_generic_args_when_matching_impls() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_const_impl_args"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
    let _label = foo.la$type_semantic$bel();
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
    let _label = foo.la$type_local$bel();
}
"#,
        &[
            AnalysisQuery::ty("const impl arg mismatch", "type_semantic"),
            AnalysisQuery::ty("local const impl arg mismatch", "type_local"),
        ],
        expect![[r#"
            const impl arg mismatch
            - <unknown>

            local const impl arg mismatch
            - <unknown>
        "#]],
    );
}

#[test]
fn uses_naive_trait_applicability_for_method_return_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_trait_applicability_type_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
    let _from_maybe = maybe_$type_maybe$user;

    let wrong = concrete.user_only();
    let _from_wrong = wro$type_wrong$ng;
}
"#,
        &[
            AnalysisQuery::ty("maybe trait method return", "type_maybe"),
            AnalysisQuery::ty("concrete trait impl mismatch", "type_wrong"),
        ],
        expect![[r#"
            maybe trait method return
            - nominal struct analysis_trait_applicability_type_at[lib]::crate::User

            concrete trait impl mismatch
            - <unknown>
        "#]],
    );
}

#[test]
fn returns_direct_trait_method_call_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_direct_trait_type_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
    let _again = i$type_direct_trait$d;
}
"#,
        &[AnalysisQuery::ty(
            "direct trait method return",
            "type_direct_trait",
        )],
        expect![[r#"
            direct trait method return
            - nominal struct analysis_direct_trait_type_at[lib]::crate::UserId
        "#]],
    );
}

#[test]
fn returns_body_local_field_access_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_field_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User {
        local_id: GlobalId,
    }

    let user: User;
    let _id: GlobalId = user.loc$type_field$al_id;
}
"#,
        &[AnalysisQuery::ty("type at body-local field", "type_field")],
        expect![[r#"
            type at body-local field
            - nominal struct analysis_body_local_field_type[lib]::crate::GlobalId
        "#]],
    );
}

#[test]
fn propagates_basic_generic_arguments_for_body_local_fields() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_generic_field_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn use_it() {
    struct Slot<T> {
        value: T,
    }

    let slot: Slot<User>;
    let _user = slot.va$type_field$lue;
}
"#,
        &[AnalysisQuery::ty("body-local generic field", "type_field")],
        expect![[r#"
            body-local generic field
            - nominal struct analysis_body_local_generic_field_type[lib]::crate::User
        "#]],
    );
}

#[test]
fn returns_body_local_method_call_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_method_type"
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

        fn again(&self) -> Self {
            missing()
        }
    }

    let user: User;
    let _id: GlobalId = user.i$type_id$d();
    let _again: User = user.a$type_again$gain();
}
"#,
        &[
            AnalysisQuery::ty("type at body-local method", "type_id"),
            AnalysisQuery::ty("type at body-local Self method", "type_again"),
        ],
        expect![[r#"
            type at body-local method
            - nominal struct analysis_body_local_method_type[lib]::crate::GlobalId

            type at body-local Self method
            - nominal struct fn analysis_body_local_method_type[lib]::crate::use_it::User
        "#]],
    );
}

#[test]
fn returns_body_local_imported_value_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_import_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    mod local {
        pub struct User;
        pub const VALUE: User = missing();
    }

    use local::User as LocalUser;
    use local::*;

    let _user: LocalUser;
    let _value = VAL$type_value$UE;
}
"#,
        &[AnalysisQuery::ty(
            "type at body-local imported value",
            "type_value",
        )],
        expect![[r#"
            type at body-local imported value
            - nominal struct fn analysis_body_local_import_type[lib]::crate::use_it::User
        "#]],
    );
}

#[test]
fn returns_nested_body_local_impl_method_call_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_nested_body_local_method_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
    let _id: GlobalId = user.i$type_id$d();
}
"#,
        &[AnalysisQuery::ty(
            "type at nested body-local method",
            "type_id",
        )],
        expect![[r#"
            type at nested body-local method
            - nominal struct analysis_nested_body_local_method_type[lib]::crate::GlobalId
        "#]],
    );
}

#[test]
fn substitutes_body_local_impl_generics_in_method_returns() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_impl_generic_method_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
    let _value: User = wrapper.ge$type_get$t();
}
"#,
        &[AnalysisQuery::ty(
            "type at body-local generic impl method",
            "type_get",
        )],
        expect![[r#"
            type at body-local generic impl method
            - nominal struct fn analysis_body_local_impl_generic_method_type[lib]::crate::use_it::User
        "#]],
    );
}

#[test]
fn propagates_enum_variant_payload_types_into_patterns() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_enum_pattern_type_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
    let _from_let = val$type_let$ue;

    let Message::User { profile } = message else { return; };
    let _from_record = pro$type_record$file;
}

pub fn match_it(maybe: Option<User>) {
    match maybe {
        Option::Some(user) => {
            let _from_match = us$type_match$er;
        }
        Option::None => {}
    }
}
"#,
        &[
            AnalysisQuery::ty("type from tuple variant let pattern", "type_let"),
            AnalysisQuery::ty("type from record variant let pattern", "type_record"),
            AnalysisQuery::ty("type from tuple variant match pattern", "type_match"),
        ],
        expect![[r#"
            type from tuple variant let pattern
            - nominal struct analysis_enum_pattern_type_at[lib]::crate::User

            type from record variant let pattern
            - nominal struct analysis_enum_pattern_type_at[lib]::crate::Profile

            type from tuple variant match pattern
            - nominal struct analysis_enum_pattern_type_at[lib]::crate::User
        "#]],
    );
}

#[test]
fn returns_tuple_field_access_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_tuple_field_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Left;
pub struct Right;

pub struct Pair(pub Left, pub Right);

pub fn use_it(pair: Pair) {
    let _right: Right = pair.$type_tuple_field$1;
}
"#,
        &[AnalysisQuery::ty("type at tuple field", "type_tuple_field")],
        expect![[r#"
            type at tuple field
            - nominal struct analysis_tuple_field_type[lib]::crate::Right
        "#]],
    );
}

#[test]
fn returns_signature_path_and_field_declaration_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_signature_type_at"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Profile;

pub struct User {
    pub pro$type_field_decl$file: Pro$type_field_path$file,
}

pub fn make(profile: Pro$type_param$file) -> Pro$type_ret$file {
    profile
}
"#,
        &[
            AnalysisQuery::ty("type at field declaration", "type_field_decl"),
            AnalysisQuery::ty("type at field type path", "type_field_path"),
            AnalysisQuery::ty("type at parameter type", "type_param"),
            AnalysisQuery::ty("type at return type", "type_ret"),
        ],
        expect![[r#"
            type at field declaration
            - nominal struct analysis_signature_type_at[lib]::crate::Profile

            type at field type path
            - nominal struct analysis_signature_type_at[lib]::crate::Profile

            type at parameter type
            - nominal struct analysis_signature_type_at[lib]::crate::Profile

            type at return type
            - nominal struct analysis_signature_type_at[lib]::crate::Profile
        "#]],
    );
}

#[test]
fn returns_self_type_in_impl_signatures() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_impl_self_signature_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

impl User {
    pub fn new() -> Se$type_impl_self_signature$lf {
        User
    }
}
"#,
        &[AnalysisQuery::ty(
            "type at impl signature Self",
            "type_impl_self_signature",
        )],
        expect![[r#"
            type at impl signature Self
            - Self struct analysis_impl_self_signature_type[lib]::crate::User
        "#]],
    );
}

#[test]
fn returns_self_receiver_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_self_receiver_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

impl User {
    pub fn owned(self) {
        let _owned = se$type_owned_self$lf;
    }

    pub fn shared(&self) {
        let _shared = se$type_shared_self$lf;
    }

    pub fn unique(&mut self) {
        let _unique = se$type_unique_self$lf;
    }
}
"#,
        &[
            AnalysisQuery::ty("type at owned self", "type_owned_self"),
            AnalysisQuery::ty("type at shared self", "type_shared_self"),
            AnalysisQuery::ty("type at mutable self", "type_unique_self"),
        ],
        expect![[r#"
            type at owned self
            - Self struct analysis_self_receiver_type[lib]::crate::User

            type at shared self
            - &Self struct analysis_self_receiver_type[lib]::crate::User

            type at mutable self
            - &mut Self struct analysis_self_receiver_type[lib]::crate::User
        "#]],
    );
}

#[test]
fn returns_body_local_struct_types_before_module_structs() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_local_struct_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn make() {
    struct User;
    let local$type_binding$: Us$type_local_path$er = User;
    let _again: User = loc$type_local_expr$al;
}

pub fn outside() {
    let outside$type_module_binding$: User = User;
}
"#,
        &[
            AnalysisQuery::ty("type at local binding", "type_binding"),
            AnalysisQuery::ty("type at local type path", "type_local_path"),
            AnalysisQuery::ty("type at local expr", "type_local_expr"),
            AnalysisQuery::ty("type at module binding", "type_module_binding"),
        ],
        expect![[r#"
            type at local binding
            - nominal struct fn analysis_local_struct_type[lib]::crate::make::User

            type at local type path
            - nominal struct fn analysis_local_struct_type[lib]::crate::make::User

            type at local expr
            - nominal struct fn analysis_local_struct_type[lib]::crate::make::User

            type at module binding
            - nominal struct analysis_local_struct_type[lib]::crate::User
        "#]],
    );
}

#[test]
fn returns_body_local_enum_variant_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_local_enum_variant_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn make() {
    enum Action {
        Start(GlobalId),
        Stop,
    }

    let _action = Action::Sta$type_local_variant$rt(GlobalId);
}
"#,
        &[AnalysisQuery::ty(
            "type at local enum variant",
            "type_local_variant",
        )],
        expect![[r#"
            type at local enum variant
            - nominal enum fn analysis_local_enum_variant_type[lib]::crate::make::Action
        "#]],
    );
}

#[test]
fn returns_body_local_record_literal_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_local_record_literal_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn make() {
    struct User {
        id: GlobalId,
    }

    let user$type_record_binding$ = Us$type_record_literal$er { id: GlobalId };
}
"#,
        &[
            AnalysisQuery::ty("type at record binding", "type_record_binding"),
            AnalysisQuery::ty("type at record literal", "type_record_literal"),
        ],
        expect![[r#"
            type at record binding
            - nominal struct fn analysis_local_record_literal_type[lib]::crate::make::User

            type at record literal
            - nominal struct fn analysis_local_record_literal_type[lib]::crate::make::User
        "#]],
    );
}

#[test]
fn returns_scope_ordered_body_local_value_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_value_shadowing_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
        let from_fn$type_inner_fn_binding$ = value();
    };

    {
        const helper: Inner = Inner;
        let from_const$type_inner_const_binding$ = helper;
    };
}
"#,
        &[
            AnalysisQuery::ty("type at inner function result", "type_inner_fn_binding"),
            AnalysisQuery::ty("type at inner const result", "type_inner_const_binding"),
        ],
        expect![[r#"
            type at inner function result
            - nominal struct analysis_body_value_shadowing_type[lib]::crate::Inner

            type at inner const result
            - nominal struct analysis_body_value_shadowing_type[lib]::crate::Inner
        "#]],
    );
}

#[test]
fn returns_body_local_associated_item_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_assoc_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn make() {
    struct User;

    impl User {
        const DEFAULT: GlobalId = GlobalId;
        type Id = GlobalId;
    }

    let default$type_assoc_const_binding$ = User::DEFAULT;
    let typed$type_assoc_type_binding$: User::Id = GlobalId;
}
"#,
        &[
            AnalysisQuery::ty(
                "type at associated const result",
                "type_assoc_const_binding",
            ),
            AnalysisQuery::ty("type at associated type result", "type_assoc_type_binding"),
        ],
        expect![[r#"
            type at associated const result
            - nominal struct analysis_body_local_assoc_type[lib]::crate::GlobalId

            type at associated type result
            - nominal struct analysis_body_local_assoc_type[lib]::crate::GlobalId
        "#]],
    );
}

#[test]
fn returns_parent_body_local_associated_item_types_from_nested_body() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_nested_body_parent_assoc_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn make() {
    struct User;

    impl User {
        const DEFAULT: GlobalId = GlobalId;
        type Id = GlobalId;
    }

    fn helper() {
        let default$type_nested_assoc_const$ = User::DEFAULT;
        let typed$type_nested_assoc_type$: User::Id = GlobalId;
    }
}
"#,
        &[
            AnalysisQuery::ty(
                "type at nested associated const result",
                "type_nested_assoc_const",
            ),
            AnalysisQuery::ty(
                "type at nested associated type result",
                "type_nested_assoc_type",
            ),
        ],
        expect![[r#"
            type at nested associated const result
            - nominal struct analysis_nested_body_parent_assoc_type[lib]::crate::GlobalId

            type at nested associated type result
            - nominal struct analysis_nested_body_parent_assoc_type[lib]::crate::GlobalId
        "#]],
    );
}

#[test]
fn returns_body_local_enum_pattern_payload_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_enum_pattern_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct GlobalId;

pub fn make() {
    enum Action {
        Start(User),
        Named { id: GlobalId },
    }

    let action: Action = Action::Start(User);
    let Action::Start(user$type_tuple_payload$) = action;
    let named: Action = Action::Start(User);
    let Action::Named { id$type_record_payload$ } = named;
}
"#,
        &[
            AnalysisQuery::ty("type at tuple payload", "type_tuple_payload"),
            AnalysisQuery::ty("type at record payload", "type_record_payload"),
        ],
        expect![[r#"
            type at tuple payload
            - nominal struct analysis_body_local_enum_pattern_type[lib]::crate::User

            type at record payload
            - nominal struct analysis_body_local_enum_pattern_type[lib]::crate::GlobalId
        "#]],
    );
}

#[test]
fn returns_body_let_annotation_types_with_body_context() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_annotation_type"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

impl User {
    pub fn capture(&self) {
        let _this: Se$type_body_self$lf = self;
    }
}

pub fn make() {
    struct User;
    let _: Us$type_wildcard_type$er = User;
    let (_left, _right): (Us$type_tuple_left$er, Us$type_tuple_right$er) = User;
}
"#,
        &[
            AnalysisQuery::ty("type at body Self annotation", "type_body_self"),
            AnalysisQuery::ty("type at wildcard annotation", "type_wildcard_type"),
            AnalysisQuery::ty("type at tuple annotation left", "type_tuple_left"),
            AnalysisQuery::ty("type at tuple annotation right", "type_tuple_right"),
        ],
        expect![[r#"
            type at body Self annotation
            - Self struct analysis_body_annotation_type[lib]::crate::User

            type at wildcard annotation
            - nominal struct fn analysis_body_annotation_type[lib]::crate::make::User

            type at tuple annotation left
            - nominal struct fn analysis_body_annotation_type[lib]::crate::make::User

            type at tuple annotation right
            - nominal struct fn analysis_body_annotation_type[lib]::crate::make::User
        "#]],
    );
}
