use expect_test::expect;

use crate::tests::utils::check_analysis;
#[test]
fn completes_unqualified_values_from_lexical_and_module_scope() {
    check_analysis(
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
    let _selected = inp$0; // complete("unqualified value completions")
    let later_user = local_user;
}
"#,
        expect![[r#"
            // unqualified value completions
            struct User
            variable input_user
            variable local_user
            fn make_user
            fn use_it
        "#]],
    );
}

#[test]
fn unqualified_local_values_shadow_module_values() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_unqualified_shadow_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn shadowed() {}

pub fn use_it(shadowed: u8) {
    let _ = sha$0; // complete("shadowed value completions")
}
"#,
        expect![[r#"
            // shadowed value completions
            variable shadowed
            fn use_it
        "#]],
    );
}

#[test]
fn sorts_unqualified_body_values_by_lexical_proximity() {
    check_analysis(
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
        c$0; // complete_verbose("unqualified value proximity")
    }
}
"#,
        expect![[r#"
            // unqualified value proximity
            variable c_z_inner
              detail: let c_z_inner: u8
              sort: 00-body:0000|c_z_inner|07|00|Declaration(DeclarationRef { kind: "binding", body: FunctionBodyRef { crate_ref: CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }, body: BodyId(1) }, binding: BindingId(1) })
              replace: 107..108
            variable c_a_outer
              detail: let c_a_outer: u8
              sort: 00-body:0002|c_a_outer|07|00|Declaration(DeclarationRef { kind: "binding", body: FunctionBodyRef { crate_ref: CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }, body: BodyId(1) }, binding: BindingId(0) })
              replace: 107..108
            fn c_module_item
              detail: pub fn c_module_item()
              sort: 01-module|c_module_item|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(0) })
              replace: 107..108
              snippet: c_module_item()$0
            fn use_it
              detail: pub fn use_it(c_a_outer: u8)
              sort: 01-module|use_it|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(1) })
              replace: 107..108
              snippet: use_it(${1:c_a_outer})$0
            keyword crate
              detail: keyword crate
              sort: ~keyword:00:crate
              replace: 107..108
              snippet: crate::$0
        "#]],
    );
}

#[test]
fn sorts_unqualified_body_types_by_lexical_proximity() {
    check_analysis(
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

    let _value: A$0; // complete_verbose("unqualified type proximity")
}
"#,
        expect![[r#"
            // unqualified type proximity
            struct ZLocal
              detail: struct ZLocal
              sort: 00-body:0000|00|ZLocal|00|Declaration(DeclarationRef { kind: "item", item: TypeDef(TypeDefRef { origin: Body(BodyRef { crate_ref: CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }, body: BodyId(0) }), id: Struct(StructId(0)) }) })
              replace: 112..113
            struct AModule
              detail: struct AModule
              sort: 01-module|00|AModule|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 112..113
            trait ATrait
              detail: trait ATrait
              sort: 01-module|01|ATrait|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(0) } })
              replace: 112..113
            module a_mod
              detail: mod a_mod
              sort: 01-module|02|a_mod|00|Declaration(DeclarationRef { kind: "module", module: ModuleRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), module: ModuleId(1) } })
              replace: 112..113
        "#]],
    );
}

#[test]
fn completes_unqualified_types_from_body_and_module_scope() {
    check_analysis(
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

    let _value: Lo$0; // complete("unqualified type completions")
}
"#,
        expect![[r#"
            // unqualified type completions
            struct LocalUser
            struct ModuleUser
        "#]],
    );
}

#[test]
fn completes_type_paths_and_owner_generics_in_item_signatures() {
    check_analysis(
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
    pub field: Wrapper<Fi$0>, // complete("struct field generic arg")
    pub current: T$0, // complete("struct field type param")
}

pub trait Service<T> {
    fn map<U>(&self, input: Wrapper<Fi$0>) -> Self$1; // complete("trait method generic arg"), complete("trait method Self")
}

impl<T> Holder<T> {
    pub fn build<const N: usize>(
        input: Wrapper<Fi$0>, // complete("impl method generic arg")
    ) -> crate::Fi$0 { // complete("qualified return type")
        loop {}
    }
}

pub fn top<T, const N: usize>(
    input: Wrapper<Fi$0>, // complete("function generic arg")
    value: T$0, // complete("function type param")
    array: Array<N$0>, // complete("function const arg")
) {}

pub fn incomplete(value: Wrapper<Fi$0 // complete("incomplete function generic arg")
"#,
        expect![[r#"
            // struct field generic arg
            struct Array
            struct Fixture
            struct Holder
            trait Service
            type_parameter T
            struct Wrapper

            // struct field type param
            struct Array
            struct Fixture
            struct Holder
            trait Service
            type_parameter T
            struct Wrapper

            // trait method generic arg
            struct Array
            struct Fixture
            struct Holder
            type_parameter Self
            trait Service
            type_parameter T
            type_parameter U
            struct Wrapper

            // trait method Self
            struct Array
            struct Fixture
            struct Holder
            type_parameter Self
            trait Service
            type_parameter T
            type_parameter U
            struct Wrapper

            // impl method generic arg
            struct Array
            struct Fixture
            struct Holder
            const N
            type_parameter Self
            trait Service
            type_parameter T
            struct Wrapper

            // qualified return type
            struct Array
            struct Fixture
            struct Holder
            trait Service
            struct Wrapper

            // function generic arg
            struct Array
            struct Fixture
            struct Holder
            const N
            trait Service
            type_parameter T
            struct Wrapper

            // function type param
            struct Array
            struct Fixture
            struct Holder
            trait Service
            type_parameter T
            struct Wrapper

            // function const arg
            struct Array
            struct Fixture
            struct Holder
            const N
            trait Service
            type_parameter T
            struct Wrapper

            // incomplete function generic arg
            struct Array
            struct Fixture
            struct Holder
            trait Service
            struct Wrapper
        "#]],
    );
}

#[test]
fn completes_local_item_signatures_from_the_local_generic_scope() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_local_signature_type_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn outer<Outer>() {
    fn inner<Inner>(value: Inn$0) {} // complete("local item generic")
}
"#,
        expect![[r#"
            type_parameter Inner // local item generic
        "#]],
    );
}

#[test]
fn completes_owner_generics_in_body_type_positions() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_generic_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Array<const N: usize>;

pub fn use_it<T, const N: usize>() {
    let _: T$0; // complete("body type parameter")
    let _: Array<N$0>; // complete("body const generic argument")
}
"#,
        expect![[r#"
            // body type parameter
            struct Array
            type_parameter T

            // body const generic argument
            struct Array
            const N
            type_parameter T
        "#]],
    );
}

#[test]
fn completes_primitive_types_in_unqualified_type_positions() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_primitive_type_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Vec<T>(T);

pub fn use_it() {
    let _value: u$0; // complete("primitive type completions")
    let _values: Vec<u$0>; // complete("primitive generic arg completions")
}
"#,
        expect![[r#"
            // primitive type completions
            struct Vec
            primitive_type u128
            primitive_type u16
            primitive_type u32
            primitive_type u64
            primitive_type u8
            primitive_type usize

            // primitive generic arg completions
            struct Vec
            primitive_type u128
            primitive_type u16
            primitive_type u32
            primitive_type u64
            primitive_type u8
            primitive_type usize
        "#]],
    );
}

#[test]
fn suppresses_shadowed_primitive_type_completions() {
    check_analysis(
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
    let _module: us$0; // complete("module shadowed primitive completions")
    let _local: u8$0; // complete("local shadowed primitive completions")
}
"#,
        expect![[r#"
            // module shadowed primitive completions
            struct u8
            struct usize

            // local shadowed primitive completions
            struct u8
            struct usize
        "#]],
    );
}

#[test]
fn completes_primitive_types_shadowed_only_by_modules() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_module_named_like_primitive_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod usize {}

pub fn use_it() {
    let _value: us$0; // complete_verbose("module named like primitive completions")
}
"#,
        expect![[r#"
            // module named like primitive completions
            module usize
              detail: mod usize
              sort: 01-module|02|usize|00|Declaration(DeclarationRef { kind: "module", module: ModuleRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), module: ModuleId(1) } })
              replace: 52..54
            primitive_type usize
              detail: primitive type usize
              sort: 02-primitive|00|usize|00|PrimitiveType(UnsignedInt(Usize))
              replace: 52..54
        "#]],
    );
}

#[test]
fn completes_more_body_local_type_and_value_items() {
    check_analysis(
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

    let _typed: Loc$0; // complete("body-local type item completions")
    let _value = loc$0; // complete("body-local value item completions")
}
"#,
        expect![[r#"
            // body-local type item completions
            struct GlobalId
            enum LocalAction
            type_alias LocalAlias
            union LocalBits
            trait LocalNamed
            struct LocalUnit

            // body-local value item completions
            struct GlobalId
            struct LocalUnit
            variable _typed
            static local_current
            const local_default
            fn local_helper
            fn use_it
        "#]],
    );
}

#[test]
fn completes_unqualified_type_args_in_generic_type_paths() {
    check_analysis(
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
    let _values: collections::Map<String, S$0>; // complete("second generic arg completions").in_lib("analysis_unqualified_generic_type_arg_completions")
    let _variant = Maybe::<S$0>::None; // complete("value path generic arg completions").in_lib("analysis_unqualified_generic_type_arg_completions")
    let _keys: collections::Map<S$0 // complete("first generic arg completions").in_lib("analysis_unqualified_generic_type_arg_completions")
}
"#,
        expect![[r#"
            // second generic arg completions
            enum Maybe
            struct Session
            struct State
            struct String
            module collections

            // value path generic arg completions
            enum Maybe
            struct Session
            struct State
            struct String
            module collections

            // first generic arg completions
            enum Maybe
            struct Session
            struct State
            struct String
            module collections
        "#]],
    );
}

#[test]
fn sorts_unqualified_type_context_completions_by_type_likelihood() {
    check_analysis(
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
    let _value: Z$0; // complete_verbose("unqualified type sorting")
}
"#,
        expect![[r#"
            // unqualified type sorting
            struct Zed
              detail: struct Zed
              sort: 01-module|00|Zed|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 89..90
            trait Mid
              detail: trait Mid
              sort: 01-module|01|Mid|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(0) } })
              replace: 89..90
            module aa_prefix
              detail: mod aa_prefix
              sort: 01-module|02|aa_prefix|00|Declaration(DeclarationRef { kind: "module", module: ModuleRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), module: ModuleId(1) } })
              replace: 89..90
        "#]],
    );
}
