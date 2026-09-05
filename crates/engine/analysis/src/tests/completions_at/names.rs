use expect_test::expect;

use super::super::utils::{AnalysisQuery, check_analysis_queries};
#[test]
fn completes_unqualified_values_from_lexical_and_module_scope() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_value_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn make_user() -> User {
    User
}

pub fn use_it(input_user: User) {
    let local_user = input_user;
    let _selected = inp$0;
    let later_user = local_user;
}
"#,
        &[AnalysisQuery::complete(
            "unqualified value completions",
            "0",
        )],
        expect![[r#"
            unqualified value completions
            - struct User
            - variable input_user
            - variable local_user
            - fn make_user
            - fn use_it
        "#]],
    );
}

#[test]
fn unqualified_local_values_shadow_module_values() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_shadow_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn shadowed() {}

pub fn use_it(shadowed: u8) {
    let _ = sha$0;
}
"#,
        &[AnalysisQuery::complete("shadowed value completions", "0")],
        expect![[r#"
            shadowed value completions
            - variable shadowed
            - fn use_it
        "#]],
    );
}

#[test]
fn sorts_unqualified_body_values_by_lexical_proximity() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_value_proximity"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn c_module_item() {}

pub fn use_it(c_a_outer: u8) {
    {
        let c_z_inner = c_a_outer;
        c$0;
    }
}
"#,
        &[AnalysisQuery::complete_verbose(
            "unqualified value proximity",
            "0",
        )],
        expect![[r#"
            unqualified value proximity
            - variable c_z_inner
              detail: let c_z_inner: u8
              sort: 00-body:0000|c_z_inner|07|00|Declaration(DeclarationRef { kind: "binding", body: FunctionBodyRef { crate_ref: CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }, body: BodyId(1) }, binding: BindingId(1) })
              replace: 107..108
            - variable c_a_outer
              detail: let c_a_outer: u8
              sort: 00-body:0002|c_a_outer|07|00|Declaration(DeclarationRef { kind: "binding", body: FunctionBodyRef { crate_ref: CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }, body: BodyId(1) }, binding: BindingId(0) })
              replace: 107..108
            - fn c_module_item
              detail: pub fn c_module_item()
              sort: 01-module|c_module_item|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(0) })
              replace: 107..108
              snippet: c_module_item()$0
            - fn use_it
              detail: pub fn use_it(c_a_outer: u8)
              sort: 01-module|use_it|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(1) })
              replace: 107..108
              snippet: use_it(${1:c_a_outer})$0
            - keyword crate
              detail: keyword crate
              sort: ~keyword:00:crate
              replace: 107..108
              snippet: crate::$0
        "#]],
    );
}

#[test]
fn sorts_unqualified_body_types_by_lexical_proximity() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_type_proximity"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod a_mod {}
pub trait ATrait {}
pub struct AModule;

pub fn use_it() {
    struct ZLocal;

    let _value: A$0;
}
"#,
        &[AnalysisQuery::complete_verbose(
            "unqualified type proximity",
            "0",
        )],
        expect![[r#"
            unqualified type proximity
            - struct ZLocal
              detail: struct ZLocal
              sort: 00-body:0000|00|ZLocal|00|Declaration(DeclarationRef { kind: "item", item: TypeDef(TypeDefRef { origin: Body(BodyRef { crate_ref: CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }, body: BodyId(0) }), id: Struct(StructId(0)) }) })
              replace: 112..113
            - struct AModule
              detail: struct AModule
              sort: 01-module|00|AModule|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 112..113
            - trait ATrait
              detail: trait ATrait
              sort: 01-module|01|ATrait|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(0) } })
              replace: 112..113
            - module a_mod
              detail: mod a_mod
              sort: 01-module|02|a_mod|00|Declaration(DeclarationRef { kind: "module", module: ModuleRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), module: ModuleId(1) } })
              replace: 112..113
        "#]],
    );
}

#[test]
fn completes_unqualified_types_from_body_and_module_scope() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_type_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct ModuleUser;

pub fn use_it() {
    struct LocalUser {
        id: u8,
    }

    let _value: Lo$0;
}
"#,
        &[AnalysisQuery::complete("unqualified type completions", "0")],
        expect![[r#"
            unqualified type completions
            - struct LocalUser
            - struct ModuleUser
        "#]],
    );
}

#[test]
fn completes_type_paths_and_owner_generics_in_item_signatures() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_signature_type_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Fixture;
pub struct Wrapper<T>(T);
pub struct Array<const N: usize>;

pub struct Holder<T> {
    pub field: Wrapper<Fi$field_arg$>,
    pub current: T$field_param$,
}

pub trait Service<T> {
    fn map<U>(&self, input: Wrapper<Fi$trait_arg$>) -> Self$trait_self$;
}

impl<T> Holder<T> {
    pub fn build<const N: usize>(
        input: Wrapper<Fi$impl_arg$>,
    ) -> crate::Fi$qualified_return$ {
        loop {}
    }
}

pub fn top<T, const N: usize>(
    input: Wrapper<Fi$function_arg$>,
    value: T$type_param$,
    array: Array<N$const_arg$>,
) {}

pub fn incomplete(value: Wrapper<Fi$incomplete$
"#,
        &[
            AnalysisQuery::complete("struct field generic arg", "field_arg"),
            AnalysisQuery::complete("struct field type param", "field_param"),
            AnalysisQuery::complete("trait method generic arg", "trait_arg"),
            AnalysisQuery::complete("trait method Self", "trait_self"),
            AnalysisQuery::complete("impl method generic arg", "impl_arg"),
            AnalysisQuery::complete("qualified return type", "qualified_return"),
            AnalysisQuery::complete("function generic arg", "function_arg"),
            AnalysisQuery::complete("function type param", "type_param"),
            AnalysisQuery::complete("function const arg", "const_arg"),
            AnalysisQuery::complete("incomplete function generic arg", "incomplete"),
        ],
        expect![[r#"
            struct field generic arg
            - struct Array
            - struct Fixture
            - struct Holder
            - trait Service
            - type_parameter T
            - struct Wrapper

            struct field type param
            - struct Array
            - struct Fixture
            - struct Holder
            - trait Service
            - type_parameter T
            - struct Wrapper

            trait method generic arg
            - struct Array
            - struct Fixture
            - struct Holder
            - type_parameter Self
            - trait Service
            - type_parameter T
            - type_parameter U
            - struct Wrapper

            trait method Self
            - struct Array
            - struct Fixture
            - struct Holder
            - type_parameter Self
            - trait Service
            - type_parameter T
            - type_parameter U
            - struct Wrapper

            impl method generic arg
            - struct Array
            - struct Fixture
            - struct Holder
            - const N
            - type_parameter Self
            - trait Service
            - type_parameter T
            - struct Wrapper

            qualified return type
            - struct Array
            - struct Fixture
            - struct Holder
            - trait Service
            - struct Wrapper

            function generic arg
            - struct Array
            - struct Fixture
            - struct Holder
            - const N
            - trait Service
            - type_parameter T
            - struct Wrapper

            function type param
            - struct Array
            - struct Fixture
            - struct Holder
            - trait Service
            - type_parameter T
            - struct Wrapper

            function const arg
            - struct Array
            - struct Fixture
            - struct Holder
            - const N
            - trait Service
            - type_parameter T
            - struct Wrapper

            incomplete function generic arg
            - struct Array
            - struct Fixture
            - struct Holder
            - trait Service
            - struct Wrapper
        "#]],
    );
}

#[test]
fn completes_local_item_signatures_from_the_local_generic_scope() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_local_signature_type_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn outer<Outer>() {
    fn inner<Inner>(value: Inn$0) {}
}
"#,
        &[AnalysisQuery::complete("local item generic", "0")],
        expect![[r#"
            local item generic
            - type_parameter Inner
        "#]],
    );
}

#[test]
fn completes_owner_generics_in_body_type_positions() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_generic_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Array<const N: usize>;

pub fn use_it<T, const N: usize>() {
    let _: T$type_param$;
    let _: Array<N$const_arg$>;
}
"#,
        &[
            AnalysisQuery::complete("body type parameter", "type_param"),
            AnalysisQuery::complete("body const generic argument", "const_arg"),
        ],
        expect![[r#"
            body type parameter
            - struct Array
            - type_parameter T

            body const generic argument
            - struct Array
            - const N
            - type_parameter T
        "#]],
    );
}

#[test]
fn completes_primitive_types_in_unqualified_type_positions() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_primitive_type_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Vec<T>(T);

pub fn use_it() {
    let _value: u$primitive$;
    let _values: Vec<u$generic_arg$>;
}
"#,
        &[
            AnalysisQuery::complete("primitive type completions", "primitive"),
            AnalysisQuery::complete("primitive generic arg completions", "generic_arg"),
        ],
        expect![[r#"
            primitive type completions
            - struct Vec
            - primitive_type u128
            - primitive_type u16
            - primitive_type u32
            - primitive_type u64
            - primitive_type u8
            - primitive_type usize

            primitive generic arg completions
            - struct Vec
            - primitive_type u128
            - primitive_type u16
            - primitive_type u32
            - primitive_type u64
            - primitive_type u8
            - primitive_type usize
        "#]],
    );
}

#[test]
fn suppresses_shadowed_primitive_type_completions() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_shadowed_primitive_type_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct usize;

pub fn use_it() {
    struct u8;
    let _module: us$module_shadow$;
    let _local: u8$local_shadow$;
}
"#,
        &[
            AnalysisQuery::complete("module shadowed primitive completions", "module_shadow"),
            AnalysisQuery::complete("local shadowed primitive completions", "local_shadow"),
        ],
        expect![[r#"
            module shadowed primitive completions
            - struct u8
            - struct usize

            local shadowed primitive completions
            - struct u8
            - struct usize
        "#]],
    );
}

#[test]
fn completes_primitive_types_shadowed_only_by_modules() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_module_named_like_primitive_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod usize {}

pub fn use_it() {
    let _value: us$module_name$;
}
"#,
        &[AnalysisQuery::complete_verbose(
            "module named like primitive completions",
            "module_name",
        )],
        expect![[r#"
            module named like primitive completions
            - module usize
              detail: mod usize
              sort: 01-module|02|usize|00|Declaration(DeclarationRef { kind: "module", module: ModuleRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), module: ModuleId(1) } })
              replace: 52..54
            - primitive_type usize
              detail: primitive type usize
              sort: 02-primitive|00|usize|00|PrimitiveType(UnsignedInt(Usize))
              replace: 52..54
        "#]],
    );
}

#[test]
fn completes_more_body_local_type_and_value_items() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_more_body_local_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct LocalUnit;
    enum LocalAction { Start }
    union LocalBits { id: GlobalId }
    type LocalAlias = GlobalId;
    trait LocalNamed {}
    const local_default: LocalAlias = GlobalId;
    static local_current: GlobalId = GlobalId;
    fn local_helper() -> LocalAlias {
        GlobalId
    }

    let _typed: Loc$type$;
    let _value = loc$value$;
}
"#,
        &[
            AnalysisQuery::complete("body-local type item completions", "type"),
            AnalysisQuery::complete("body-local value item completions", "value"),
        ],
        expect![[r#"
            body-local type item completions
            - struct GlobalId
            - enum LocalAction
            - type_alias LocalAlias
            - union LocalBits
            - trait LocalNamed
            - struct LocalUnit

            body-local value item completions
            - struct GlobalId
            - struct LocalUnit
            - variable _typed
            - static local_current
            - const local_default
            - fn local_helper
            - fn use_it
        "#]],
    );
}

#[test]
fn completes_unqualified_type_args_in_generic_type_paths() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_generic_type_arg_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Session;
pub struct State;
pub struct String;

pub mod collections {
    pub struct Map<K, V>;
}

pub enum Maybe<T> {
    Some(T),
    None,
}

pub fn use_it() {
    let _values: collections::Map<String, S$value_arg$>;
    let _variant = Maybe::<S$value_path_arg$>::None;
    let _keys: collections::Map<S$key_arg$
}
"#,
        &[
            AnalysisQuery::complete("first generic arg completions", "key_arg")
                .in_lib("analysis_unqualified_generic_type_arg_completions"),
            AnalysisQuery::complete("second generic arg completions", "value_arg")
                .in_lib("analysis_unqualified_generic_type_arg_completions"),
            AnalysisQuery::complete("value path generic arg completions", "value_path_arg")
                .in_lib("analysis_unqualified_generic_type_arg_completions"),
        ],
        expect![[r#"
            first generic arg completions
            - enum Maybe
            - struct Session
            - struct State
            - struct String
            - module collections

            second generic arg completions
            - enum Maybe
            - struct Session
            - struct State
            - struct String
            - module collections

            value path generic arg completions
            - enum Maybe
            - struct Session
            - struct State
            - struct String
            - module collections
        "#]],
    );
}

#[test]
fn sorts_unqualified_type_context_completions_by_type_likelihood() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_type_sorting"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod aa_prefix {}
pub trait Mid {}
pub struct Zed;

pub fn use_it() {
    let _value: Z$0;
}
"#,
        &[AnalysisQuery::complete_verbose(
            "unqualified type sorting",
            "0",
        )],
        expect![[r#"
            unqualified type sorting
            - struct Zed
              detail: struct Zed
              sort: 01-module|00|Zed|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 89..90
            - trait Mid
              detail: trait Mid
              sort: 01-module|01|Mid|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(0) } })
              replace: 89..90
            - module aa_prefix
              detail: mod aa_prefix
              sort: 01-module|02|aa_prefix|00|Declaration(DeclarationRef { kind: "module", module: ModuleRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), module: ModuleId(1) } })
              replace: 89..90
        "#]],
    );
}
