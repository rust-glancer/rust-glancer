//! Analysis-level inference regression tests.
//!
//! While inference is not an inherent analysis property, it is a dedicated scope that affects
//! analysis a lot. This module encapsulates tests that ensure inference behavior in a wider
//! analysis context.

use expect_test::expect;

use super::utils::{AnalysisQuery, check_analysis_queries};

#[test]
fn refines_unsuffixed_numeric_literals_from_let_annotations() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_numeric_literal_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    let default_int = 1$type_default_int$;
    let annotated_int: u64 = 1$type_u64$;
    let default_float = 1.0$type_default_float$;
    let annotated_float: f32 = 1.0$type_f32$;
    let mismatch: bool = 1$type_mismatch$;
}
"#,
        &[
            AnalysisQuery::ty("default integer literal", "type_default_int"),
            AnalysisQuery::ty("annotated integer literal", "type_u64"),
            AnalysisQuery::ty("default float literal", "type_default_float"),
            AnalysisQuery::ty("annotated float literal", "type_f32"),
            AnalysisQuery::ty("mismatched numeric literal", "type_mismatch"),
        ],
        expect![[r#"
            default integer literal
            - i32

            annotated integer literal
            - u64

            default float literal
            - f64

            annotated float literal
            - f32

            mismatched numeric literal
            - <unknown>
        "#]],
    );
}

#[test]
fn propagates_let_annotation_expected_types_through_tuple_expressions() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_tuple_expected_type_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    let default_pair = (1, 1.0)$type_default_pair$;
    let annotated_pair: (u64, f32) = (1$type_pair_int$, 1.0$type_pair_float$)$type_pair$;
    let nested: (u64, (f32, bool)) = (1$type_nested_int$, (1.0$type_nested_float$, true$type_nested_bool$)$type_nested_inner$)$type_nested$;
    let conflict: (bool, f32) = (1$type_conflict_int$, 1.0$type_conflict_float$)$type_conflict_pair$;
}
"#,
        &[
            AnalysisQuery::ty("default tuple expression", "type_default_pair"),
            AnalysisQuery::ty("annotated tuple integer field", "type_pair_int"),
            AnalysisQuery::ty("annotated tuple float field", "type_pair_float"),
            AnalysisQuery::ty("annotated tuple expression", "type_pair"),
            AnalysisQuery::ty("nested tuple integer field", "type_nested_int"),
            AnalysisQuery::ty("nested tuple float field", "type_nested_float"),
            AnalysisQuery::ty("nested tuple bool field", "type_nested_bool"),
            AnalysisQuery::ty("nested inner tuple expression", "type_nested_inner"),
            AnalysisQuery::ty("nested tuple expression", "type_nested"),
            AnalysisQuery::ty("conflicting tuple integer field", "type_conflict_int"),
            AnalysisQuery::ty("conflicting tuple float field", "type_conflict_float"),
            AnalysisQuery::ty("conflicting tuple expression", "type_conflict_pair"),
        ],
        expect![[r#"
            default tuple expression
            - (i32, f64)

            annotated tuple integer field
            - u64

            annotated tuple float field
            - f32

            annotated tuple expression
            - (u64, f32)

            nested tuple integer field
            - u64

            nested tuple float field
            - f32

            nested tuple bool field
            - bool

            nested inner tuple expression
            - (f32, bool)

            nested tuple expression
            - (u64, (f32, bool))

            conflicting tuple integer field
            - <unknown>

            conflicting tuple float field
            - f32

            conflicting tuple expression
            - (<unknown>, f32)
        "#]],
    );
}

#[test]
fn propagates_expected_types_through_transparent_and_array_expressions() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_shape_expected_type_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn missing<T>() -> T {}

pub fn use_it() {
    let paren: User = (missing()$type_paren_inner$)$type_paren$;
    let shared: &u64 = &1$type_ref_inner$;
    let values: [u64; 2] = [1$type_array_left$, 2$type_array_right$]$type_array$;
    let repeated: [u64; 2] = [1$type_repeat_inner$; 2]$type_repeat$;
    let users: [User; 1] = [missing()$type_array_user$]$type_array_generic$;
}
"#,
        &[
            AnalysisQuery::ty("paren generic call inner", "type_paren_inner"),
            AnalysisQuery::ty("paren generic call expression", "type_paren"),
            AnalysisQuery::ty("reference integer inner", "type_ref_inner"),
            AnalysisQuery::ty("array left integer", "type_array_left"),
            AnalysisQuery::ty("array right integer", "type_array_right"),
            AnalysisQuery::ty("array expression", "type_array"),
            AnalysisQuery::ty("repeat array integer", "type_repeat_inner"),
            AnalysisQuery::ty("repeat array expression", "type_repeat"),
            AnalysisQuery::ty("array generic call", "type_array_user"),
            AnalysisQuery::ty("array generic expression", "type_array_generic"),
        ],
        expect![[r#"
            paren generic call inner
            - nominal struct analysis_shape_expected_type_inference[lib]::crate::User

            paren generic call expression
            - nominal struct analysis_shape_expected_type_inference[lib]::crate::User

            reference integer inner
            - u64

            array left integer
            - u64

            array right integer
            - u64

            array expression
            - [u64; 2]

            repeat array integer
            - u64

            repeat array expression
            - [u64; 2]

            array generic call
            - nominal struct analysis_shape_expected_type_inference[lib]::crate::User

            array generic expression
            - [nominal struct analysis_shape_expected_type_inference[lib]::crate::User; 1]
        "#]],
    );
}

#[test]
fn propagates_function_return_expected_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_return_expected_type_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn int_tail() -> u64 {
    1$type_int_tail$
}

pub fn float_tail() -> f32 {
    1.0$type_float_tail$
}

pub fn tuple_tail() -> (u64, f32) {
    (1$type_tuple_int$, 1.0$type_tuple_float$)$type_tuple_tail$
}

pub fn explicit_return(flag: bool) -> u64 {
    if flag {
        return 1$type_return$;
    }
    2$type_final_tail$
}
"#,
        &[
            AnalysisQuery::ty("integer tail return", "type_int_tail"),
            AnalysisQuery::ty("float tail return", "type_float_tail"),
            AnalysisQuery::ty("tuple return integer field", "type_tuple_int"),
            AnalysisQuery::ty("tuple return float field", "type_tuple_float"),
            AnalysisQuery::ty("tuple tail return", "type_tuple_tail"),
            AnalysisQuery::ty("explicit return expression", "type_return"),
            AnalysisQuery::ty("final tail return", "type_final_tail"),
        ],
        expect![[r#"
            integer tail return
            - u64

            float tail return
            - f32

            tuple return integer field
            - u64

            tuple return float field
            - f32

            tuple tail return
            - (u64, f32)

            explicit return expression
            - u64

            final tail return
            - u64
        "#]],
    );
}

#[test]
fn propagates_function_call_argument_expected_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_call_argument_expected_type_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn takes_u64(value: u64) {}

pub fn takes_f32(value: f32) {}

pub fn takes_pair(value: (u64, f32)) {}

pub fn use_it() {
    takes_u64(1$type_u64$);
    takes_f32(1.0$type_f32$);
    takes_pair((1$type_pair_int$, 1.0$type_pair_float$)$type_pair$);
}
"#,
        &[
            AnalysisQuery::ty("integer call argument", "type_u64"),
            AnalysisQuery::ty("float call argument", "type_f32"),
            AnalysisQuery::ty("tuple call integer field", "type_pair_int"),
            AnalysisQuery::ty("tuple call float field", "type_pair_float"),
            AnalysisQuery::ty("tuple call argument", "type_pair"),
        ],
        expect![[r#"
            integer call argument
            - u64

            float call argument
            - f32

            tuple call integer field
            - u64

            tuple call float field
            - f32

            tuple call argument
            - (u64, f32)
        "#]],
    );
}

#[test]
fn propagates_method_call_argument_expected_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_method_call_argument_expected_type_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Sink;

impl Sink {
    pub fn put_u64(&self, value: u64) {}

    pub fn put_f32(&self, value: f32) {}

    pub fn put_pair(&self, value: (u64, f32)) {}
}

pub struct GenericSink<T> {
    value: T,
}

impl<T> GenericSink<T> {
    pub fn put(&self, value: T) {}

    pub fn put_pair(&self, value: (T, f32)) {}
}

pub fn use_it(sink: Sink, generic_sink: GenericSink<u64>) {
    sink.put_u64(1$type_u64$);
    sink.put_f32(1.0$type_f32$);
    sink.put_pair((1$type_pair_int$, 1.0$type_pair_float$)$type_pair$);

    generic_sink.put(1$type_generic$);
    generic_sink.put_pair((1$type_generic_pair_int$, 1.0$type_generic_pair_float$)$type_generic_pair$);
}
"#,
        &[
            AnalysisQuery::ty("integer method argument", "type_u64"),
            AnalysisQuery::ty("float method argument", "type_f32"),
            AnalysisQuery::ty("tuple method integer field", "type_pair_int"),
            AnalysisQuery::ty("tuple method float field", "type_pair_float"),
            AnalysisQuery::ty("tuple method argument", "type_pair"),
            AnalysisQuery::ty("generic receiver method argument", "type_generic"),
            AnalysisQuery::ty(
                "generic receiver tuple method integer field",
                "type_generic_pair_int",
            ),
            AnalysisQuery::ty(
                "generic receiver tuple method float field",
                "type_generic_pair_float",
            ),
            AnalysisQuery::ty(
                "generic receiver tuple method argument",
                "type_generic_pair",
            ),
        ],
        expect![[r#"
            integer method argument
            - u64

            float method argument
            - f32

            tuple method integer field
            - u64

            tuple method float field
            - f32

            tuple method argument
            - (u64, f32)

            generic receiver method argument
            - u64

            generic receiver tuple method integer field
            - u64

            generic receiver tuple method float field
            - f32

            generic receiver tuple method argument
            - (u64, f32)
        "#]],
    );
}

#[test]
fn propagates_expected_types_into_generic_call_results() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_generic_call_result_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn id<T>(value: T) -> T {}

pub fn missing<T>() -> T {}

pub fn takes_user(value: User) {}

pub struct Factory;

impl Factory {
    pub fn make<T>() -> T {}
}

pub struct Builder;

impl Builder {
    pub fn build<T>(&self) -> T {}
}

pub fn use_it(builder: Builder) {
    let annotated: User = id(missing())$type_annotated$;
    takes_user(id(missing())$type_arg$);
    let pair: (User, bool) = (id(missing())$type_pair_user$, true$type_pair_bool$)$type_pair$;
    let associated: User = Factory::make()$type_associated$;
    let method: User = builder.build()$type_method$;
    let unconstrained = id(missing())$type_unconstrained$;
}
"#,
        &[
            AnalysisQuery::ty("annotated generic call result", "type_annotated"),
            AnalysisQuery::ty("function argument generic call result", "type_arg"),
            AnalysisQuery::ty("tuple generic call field", "type_pair_user"),
            AnalysisQuery::ty("tuple bool field", "type_pair_bool"),
            AnalysisQuery::ty("tuple containing generic call", "type_pair"),
            AnalysisQuery::ty("associated function generic call result", "type_associated"),
            AnalysisQuery::ty("method generic call result", "type_method"),
            AnalysisQuery::ty("unconstrained generic call result", "type_unconstrained"),
        ],
        expect![[r#"
            annotated generic call result
            - nominal struct analysis_generic_call_result_inference[lib]::crate::User

            function argument generic call result
            - nominal struct analysis_generic_call_result_inference[lib]::crate::User

            tuple generic call field
            - nominal struct analysis_generic_call_result_inference[lib]::crate::User

            tuple bool field
            - bool

            tuple containing generic call
            - (nominal struct analysis_generic_call_result_inference[lib]::crate::User, bool)

            associated function generic call result
            - nominal struct analysis_generic_call_result_inference[lib]::crate::User

            method generic call result
            - nominal struct analysis_generic_call_result_inference[lib]::crate::User

            unconstrained generic call result
            - <unknown>
        "#]],
    );
}

#[test]
fn propagates_expected_types_into_generic_call_result_shapes() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_generic_call_result_shape_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Error;

pub struct Vec<T> {
    value: T,
}

pub struct Option<T> {
    value: T,
}

pub struct Result<T, E> {
    ok: T,
    err: E,
}

pub fn make_vec<T>() -> Vec<T> {}
pub fn make_option<T>() -> Option<T> {}
pub fn make_result<T, E>() -> Result<T, E> {}

pub struct Factory;

impl Factory {
    pub fn make_vec<T>() -> Vec<T> {}
}

pub struct Builder;

impl Builder {
    pub fn build_vec<T>(&self) -> Vec<T> {}
}

pub fn use_it(builder: Builder) {
    let free: Vec<User> = make_vec()$type_free$;
    let associated: Vec<User> = Factory::make_vec()$type_associated$;
    let method: Vec<User> = builder.build_vec()$type_method$;
    let option: Option<User> = make_option()$type_option$;
    let result: Result<User, Error> = make_result()$type_result$;
    let unconstrained = make_vec()$type_unconstrained$;
}
"#,
        &[
            AnalysisQuery::ty("free function generic return shape", "type_free"),
            AnalysisQuery::ty(
                "associated function generic return shape",
                "type_associated",
            ),
            AnalysisQuery::ty("method generic return shape", "type_method"),
            AnalysisQuery::ty("single-param generic return shape", "type_option"),
            AnalysisQuery::ty("multi-param generic return shape", "type_result"),
            AnalysisQuery::ty("unconstrained generic return shape", "type_unconstrained"),
        ],
        expect![[r#"
            free function generic return shape
            - nominal struct analysis_generic_call_result_shape_inference[lib]::crate::Vec<nominal struct analysis_generic_call_result_shape_inference[lib]::crate::User>

            associated function generic return shape
            - nominal struct analysis_generic_call_result_shape_inference[lib]::crate::Vec<nominal struct analysis_generic_call_result_shape_inference[lib]::crate::User>

            method generic return shape
            - nominal struct analysis_generic_call_result_shape_inference[lib]::crate::Vec<nominal struct analysis_generic_call_result_shape_inference[lib]::crate::User>

            single-param generic return shape
            - nominal struct analysis_generic_call_result_shape_inference[lib]::crate::Option<nominal struct analysis_generic_call_result_shape_inference[lib]::crate::User>

            multi-param generic return shape
            - nominal struct analysis_generic_call_result_shape_inference[lib]::crate::Result<nominal struct analysis_generic_call_result_shape_inference[lib]::crate::User, nominal struct analysis_generic_call_result_shape_inference[lib]::crate::Error>

            unconstrained generic return shape
            - nominal struct analysis_generic_call_result_shape_inference[lib]::crate::Vec<<unknown>>
        "#]],
    );
}

#[test]
fn propagates_associated_function_prefix_generics() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_associated_function_prefix_generic_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub struct Vec<T> {
    value: T,
}

impl<T> Vec<T> {
    pub fn new() -> Self {}
}

pub fn use_it() {
    let inferred: Vec<User> = Vec::new()$type_inferred$;
    let explicit = Vec::<User>::new()$type_explicit$;
    let wildcard = Vec::<_>::new()$type_wildcard$;
}
"#,
        &[
            AnalysisQuery::ty(
                "associated function inferred prefix generic",
                "type_inferred",
            ),
            AnalysisQuery::ty(
                "associated function explicit prefix generic",
                "type_explicit",
            ),
            AnalysisQuery::ty(
                "associated function wildcard prefix generic",
                "type_wildcard",
            ),
        ],
        expect![[r#"
            associated function inferred prefix generic
            - nominal struct analysis_associated_function_prefix_generic_inference[lib]::crate::Vec<nominal struct analysis_associated_function_prefix_generic_inference[lib]::crate::User>

            associated function explicit prefix generic
            - nominal struct analysis_associated_function_prefix_generic_inference[lib]::crate::Vec<nominal struct analysis_associated_function_prefix_generic_inference[lib]::crate::User>

            associated function wildcard prefix generic
            - nominal struct analysis_associated_function_prefix_generic_inference[lib]::crate::Vec<<unknown>>
        "#]],
    );
}

#[test]
fn carries_generic_variables_through_simple_bindings() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_simple_binding_generic_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub struct Vec<T> {
    value: T,
}

impl<T> Vec<T> {
    pub fn new() -> Self {}
}

pub fn use_it() {
    let values = Vec::new()$type_initializer$;
    let typed: Vec<User> = values$type_constrained_read$;
    let alias = values$type_alias_read$;
    let later: Vec<User> = alias$type_alias_constrained_read$;
    let wrapped = (Vec::new(),)$type_wrapped_initializer$;
    let wrapped_later: (Vec<User>,) = wrapped$type_wrapped_read$;
}
"#,
        &[
            AnalysisQuery::ty("binding initializer generic result", "type_initializer"),
            AnalysisQuery::ty(
                "binding read constrained by annotation",
                "type_constrained_read",
            ),
            AnalysisQuery::ty("binding read through alias", "type_alias_read"),
            AnalysisQuery::ty(
                "alias read constrained by annotation",
                "type_alias_constrained_read",
            ),
            AnalysisQuery::ty("wrapped binding initializer", "type_wrapped_initializer"),
            AnalysisQuery::ty("wrapped binding read", "type_wrapped_read"),
        ],
        expect![[r#"
            binding initializer generic result
            - nominal struct analysis_simple_binding_generic_inference[lib]::crate::Vec<nominal struct analysis_simple_binding_generic_inference[lib]::crate::User>

            binding read constrained by annotation
            - nominal struct analysis_simple_binding_generic_inference[lib]::crate::Vec<nominal struct analysis_simple_binding_generic_inference[lib]::crate::User>

            binding read through alias
            - nominal struct analysis_simple_binding_generic_inference[lib]::crate::Vec<nominal struct analysis_simple_binding_generic_inference[lib]::crate::User>

            alias read constrained by annotation
            - nominal struct analysis_simple_binding_generic_inference[lib]::crate::Vec<nominal struct analysis_simple_binding_generic_inference[lib]::crate::User>

            wrapped binding initializer
            - (nominal struct analysis_simple_binding_generic_inference[lib]::crate::Vec<nominal struct analysis_simple_binding_generic_inference[lib]::crate::User>,)

            wrapped binding read
            - (nominal struct analysis_simple_binding_generic_inference[lib]::crate::Vec<nominal struct analysis_simple_binding_generic_inference[lib]::crate::User>,)
        "#]],
    );
}

#[test]
fn constrains_receiver_generic_variables_from_method_arguments() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_method_receiver_generic_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub struct Vec<T> {
    value: T,
}

impl<T> Vec<T> {
    pub fn new() -> Self {}
    pub fn push(&mut self, value: T) {}
}

pub fn user_value(user: User) {
    let mut values = Vec::new()$type_user_initializer$;
    values.push(user);
    values$type_user_read$;
}

pub fn suffixed_integer() {
    let mut values = Vec::new()$type_u64_initializer$;
    values.push(10u64);
    values$type_u64_read$;
}

pub fn unsuffixed_integer() {
    let mut values = Vec::new()$type_i32_initializer$;
    values.push(10);
    values$type_i32_read$;
}
"#,
        &[
            AnalysisQuery::ty("user receiver initializer", "type_user_initializer"),
            AnalysisQuery::ty("user receiver read", "type_user_read"),
            AnalysisQuery::ty("u64 receiver initializer", "type_u64_initializer"),
            AnalysisQuery::ty("u64 receiver read", "type_u64_read"),
            AnalysisQuery::ty("i32 receiver initializer", "type_i32_initializer"),
            AnalysisQuery::ty("i32 receiver read", "type_i32_read"),
        ],
        expect![[r#"
            user receiver initializer
            - nominal struct analysis_method_receiver_generic_inference[lib]::crate::Vec<nominal struct analysis_method_receiver_generic_inference[lib]::crate::User>

            user receiver read
            - nominal struct analysis_method_receiver_generic_inference[lib]::crate::Vec<nominal struct analysis_method_receiver_generic_inference[lib]::crate::User>

            u64 receiver initializer
            - nominal struct analysis_method_receiver_generic_inference[lib]::crate::Vec<u64>

            u64 receiver read
            - nominal struct analysis_method_receiver_generic_inference[lib]::crate::Vec<u64>

            i32 receiver initializer
            - nominal struct analysis_method_receiver_generic_inference[lib]::crate::Vec<i32>

            i32 receiver read
            - nominal struct analysis_method_receiver_generic_inference[lib]::crate::Vec<i32>
        "#]],
    );
}

#[test]
fn applies_explicit_enum_prefix_generics_to_payload_expected_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_enum_prefix_generic_payload_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Slot<T> {
    Put(T),
}

pub fn use_it() {
    let slot = Slot::<u64>::Put(1$type_payload$)$type_slot$;
}
"#,
        &[
            AnalysisQuery::ty("enum variant explicit generic payload", "type_payload"),
            AnalysisQuery::ty("enum variant explicit generic result", "type_slot"),
        ],
        expect![[r#"
            enum variant explicit generic payload
            - u64

            enum variant explicit generic result
            - nominal enum analysis_enum_prefix_generic_payload_inference[lib]::crate::Slot<u64>
        "#]],
    );
}

#[test]
fn propagates_record_field_initializer_expected_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_record_field_expected_type_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Pair {
    left: u64,
    right: f32,
    nested: (u64, f32),
}

pub fn use_it() {
    let _pair = Pair {
        left: 1$type_left$,
        right: 1.0$type_right$,
        nested: (1$type_nested_int$, 1.0$type_nested_float$)$type_nested$,
    };
}
"#,
        &[
            AnalysisQuery::ty("record integer field initializer", "type_left"),
            AnalysisQuery::ty("record float field initializer", "type_right"),
            AnalysisQuery::ty("record tuple integer field", "type_nested_int"),
            AnalysisQuery::ty("record tuple float field", "type_nested_float"),
            AnalysisQuery::ty("record tuple field initializer", "type_nested"),
        ],
        expect![[r#"
            record integer field initializer
            - u64

            record float field initializer
            - f32

            record tuple integer field
            - u64

            record tuple float field
            - f32

            record tuple field initializer
            - (u64, f32)
        "#]],
    );
}

#[test]
fn propagates_enum_variant_payload_expected_types() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_enum_variant_payload_expected_type_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Action {
    Set(u64, f32),
    Pair((u64, f32)),
}

pub fn use_it() {
    let _action = Action::Set(1$type_action_int$, 1.0$type_action_float$);
    let _pair = Action::Pair((1$type_pair_int$, 1.0$type_pair_float$)$type_pair$);
}
"#,
        &[
            AnalysisQuery::ty("enum variant integer payload", "type_action_int"),
            AnalysisQuery::ty("enum variant float payload", "type_action_float"),
            AnalysisQuery::ty("enum variant tuple integer field", "type_pair_int"),
            AnalysisQuery::ty("enum variant tuple float field", "type_pair_float"),
            AnalysisQuery::ty("enum variant tuple payload", "type_pair"),
        ],
        expect![[r#"
            enum variant integer payload
            - u64

            enum variant float payload
            - f32

            enum variant tuple integer field
            - u64

            enum variant tuple float field
            - f32

            enum variant tuple payload
            - (u64, f32)
        "#]],
    );
}
