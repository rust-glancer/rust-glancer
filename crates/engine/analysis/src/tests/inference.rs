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
