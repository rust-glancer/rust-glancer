use expect_test::expect;

use crate::tests::utils::check_analysis;
#[test]
fn completes_specialized_strings_macro_fragments_and_postfix_transforms() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "specialized_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
const MODULE_CAPTURE: usize = 1;
static STATIC_CAPTURE: usize = 2;

extern "C-un$0" fn foreign(); // complete_verbose_with_source("extern ABI").matching("C-unwind")

macro_rules! capture {
    ($value: ex$0) => { $value }; // complete_verbose_with_source("macro fragment").matching("expr")
}

fn complete<const COUNT: usize>(local_capture: usize, condition: bool, value: i32) {
    let nested_capture = local_capture;
    let _ = format!("{loc$0}", explicit_name = value); // complete_verbose_with_source("format local").matching("local_capture")
    let _ = format!("{exp$0}", explicit_name = value); // complete_verbose_with_source("format named argument").matching("explicit_name")
    let _ = format!("{MOD$0}"); // complete_with_source("format module const").matching("MODULE_CAPTURE")
    let _ = env!("CARGO_MAN$0"); // complete_verbose_with_source("Cargo environment").matching("CARGO_MANIFEST_DIR")
    let _ = option_env!("CARGO_BIN$0"); // complete_with_source("Cargo option environment").matching("CARGO_BIN_NAME")
    let _ = write!("ordinary$0", "{nes$1}"); // complete_with_source("write destination is ordinary").matching("nested_capture"), complete_with_source("write format").matching("nested_capture")
    let _ = (value + 1).ma$0; // complete_verbose_with_source("postfix match").matching("match")
    let _ = condition.i$0; // complete_verbose_with_source("boolean postfix").matching("if")
    let _ = value.i$0; // complete_with_source("non-boolean postfix").matching("if")
}
"#,
        expect![[r#"
            // extern ABI
            value C-unwind
              detail: extern ABI C-unwind
              sort: 00-specialized:08:C-unwind
              replace: 76..80

            // macro fragment
            value expr
              detail: macro fragment expr
              sort: 00-specialized:08:expr
              replace: 133..135
            value expr_2021
              detail: macro fragment expr_2021
              sort: 00-specialized:08:expr_2021
              replace: 133..135

            // format local
            variable local_capture
              detail: format capture local_capture
              sort: 00-body:0001|local_capture|07|00|Declaration(DeclarationRef { kind: "binding", body: FunctionBodyRef { crate_ref: CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }, body: BodyId(0) }, binding: BindingId(0) })
              replace: 302..305

            // format named argument
            variable explicit_name
              detail: format argument explicit_name
              sort: 00-specialized:07:explicit_name
              replace: 355..358

            const MODULE_CAPTURE // format module const

            // Cargo environment
            value CARGO_MANIFEST_DIR
              detail: directory containing this package manifest
              sort: 00-specialized:08:CARGO_MANIFEST_DIR
              replace: 434..443

            value CARGO_BIN_NAME // Cargo option environment
            <none> // write destination is ordinary
            variable nested_capture // write format

            // postfix match
            postfix match
              detail: postfix match expr
              sort: 00-specialized:09:match
              replace: 538..552
              snippet: match (value + 1) {
                ${1:_} => { $0 },
            }

            // boolean postfix
            postfix if
              detail: postfix if expr
              sort: 00-specialized:09:if
              replace: 566..577
              snippet: if condition {
                $0
            }

            <none> // non-boolean postfix
        "#]],
    );
}

#[test]
fn completes_attributes_lifetimes_labels_visibility_extern_and_const_sites() {
    check_analysis(
        r#"
//- /Cargo.toml
[workspace]
members = ["macros", "dependency", "app"]
resolver = "3"

//- /macros/Cargo.toml
[package]
name = "macros"
version = "0.1.0"
edition = "2024"

[lib]
proc-macro = true

//- /macros/src/lib.rs
extern crate proc_macro;

#[proc_macro_attribute]
pub fn traced(_attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    item
}

#[proc_macro_derive(Stored)]
pub fn stored(_item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    proc_macro::TokenStream::new()
}

//- /dependency/Cargo.toml
[package]
name = "dependency"
version = "0.1.0"
edition = "2024"

//- /dependency/src/lib.rs
pub struct Dependency;

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[features]
default = []
serde-support = []

[dependencies]
macros = { path = "../macros" }
dependency = { path = "../dependency" }

//- /app/src/lib.rs
const LIMIT: usize = 8;
static STATIC_LIMIT: usize = 16;

#[der$0] // complete_with_source("attribute path").in_lib("app").matching("derive")
struct AttributePath;

#[macros::tra$0] // complete_with_source("attribute proc macro").in_lib("app").matching("traced")
struct AttributeMacro;

#[derive(Debug, Cl$0)] // complete_with_source("builtin derive").in_lib("app").matching("Clone")
struct BuiltinDerive;

#[derive(macros::Sto$0)] // complete_with_source("derive proc macro").in_lib("app").matching("Stored")
struct MacroDerive;

#[allow(dead$0)] // complete_with_source("lint input").in_lib("app").matching("dead_code")
#[repr(tra$0)] // complete_with_source("repr input").in_lib("app").matching("transparent")
#[cfg(tar$0)] // complete_with_source("cfg key").in_lib("app").matching("target_arch")
#[cfg(feature = "serde-$0")] // complete_verbose_with_source("cfg Cargo feature").in_lib("app").matching("serde-support")
#[diagnostic::on_unimplemented(mes$0)] // complete_with_source("diagnostic input").in_lib("app").matching("message")
#[deprecated(si$0)] // complete_with_source("compatibility input").in_lib("app").matching("since")
struct AttributeInputs;

pub mod outer {
    pub mod inner {
        pub(in crate::outer::in$0) struct Restricted; // complete_with_source("visibility module").in_lib("app").matching("inner")
    }
}

extern crate dep$0; // complete_with_source("extern crate").in_lib("app").matching("dependency")

struct Buffer<const N: usize = LIM$0>([u8; N]); // complete_with_source("const default").in_lib("app").matching("LIMIT")
struct Array([u8; LIM$0]); // complete_with_source("array length").in_lib("app").matching("LIMIT")

fn signature_lifetime<'outer>(value: &'outer u8) -> &'out$0 u8 { // complete_with_source("lifetime").in_lib("app").matching("'outer")
    value
}

fn const_body<const N: usize>() {
    let _: [u8; N$0]; // complete_with_source("const parameter").in_lib("app").matching("N")
    let _: Buffer<{ LIM$0 }>; // complete_with_source("braced const argument").in_lib("app").matching("LIMIT")

    'outer: loop {
        'inner: loop {
            break 'inn$0; // complete_with_source("loop label").in_lib("app").matching("'inner")
        }
    }
}
"#,
        expect![[r#"
            attribute derive // attribute path
            macro traced // attribute proc macro
            value Clone // builtin derive
            macro Stored // derive proc macro
            value dead_code // lint input
            value transparent // repr input
            value target_arch // cfg key

            // cfg Cargo feature
            value serde-support
              detail: attribute input serde-support
              sort: 00-specialized:08:serde-support
              replace: 272..278

            value message // diagnostic input
            value since // compatibility input
            module inner // visibility module
            module dependency // extern crate
            const LIMIT // const default
            const LIMIT // array length
            lifetime 'outer // lifetime
            const N // const parameter
            const LIMIT // braced const argument
            label 'inner // loop label
        "#]],
    );
}

#[test]
fn completes_missing_trait_impl_members_with_instantiated_stubs() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_missing_trait_member_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Service<T> {
    /// The response selected by an implementation.
    type Output;
    type Defaulted = T;

    const REQUIRED: T;
    const DEFAULTED: T = loop {};

    /// Handle one request.
    fn required(&self, value: T) -> Self::Output;
    fn defaulted(&self) -> T { loop {} }
}

pub struct Worker;

impl Service<u8> for Worker {
    type Output = u16;
    fn defaulted(&self) -> u8 { 0 }

    req$0 // complete_verbose_with_source("missing trait members")
}
"#,
        expect![[r#"
            // missing trait members
            const REQUIRED
              detail: required trait member: const REQUIRED: u8
              sort: 00-required|REQUIRED|05|00|Declaration(DeclarationRef { kind: "item", item: Const(ConstRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: ConstId(0) }) })
              replace: 411..414
              snippet: const REQUIRED: u8 = ${1:todo!()};
            fn required
              detail: required trait member: fn required(&self, value: u8) -> Self::Output
              docs: Handle one request.
              sort: 00-required|required|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(0) })
              replace: 411..414
              snippet: fn required(&self, value: u8) -> Self::Output {
                ${1:todo!()}
            }
            const DEFAULTED
              detail: default trait member: const DEFAULTED: u8
              sort: 01-default|DEFAULTED|05|00|Declaration(DeclarationRef { kind: "item", item: Const(ConstRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: ConstId(1) }) })
              replace: 411..414
              snippet: const DEFAULTED: u8 = ${1:todo!()};
            type_alias Defaulted
              detail: default trait member: type Defaulted = u8
              sort: 01-default|Defaulted|04|00|Declaration(DeclarationRef { kind: "item", item: TypeAlias(TypeAliasRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: TypeAliasId(1) }) })
              replace: 411..414
              snippet: type Defaulted = ${1:u8};
        "#]],
    );
}

#[test]
fn trait_impl_member_introducers_filter_and_replace_the_whole_prefix() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_trait_member_introducer_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
trait Service {
    type Output;
    const LIMIT: usize;
    fn required(&self) -> Self::Output;
}

struct FunctionWorker;
impl Service for FunctionWorker {
    fn re$0 // complete_verbose_with_source("function trait member prefix")
}

struct TypeWorker;
impl Service for TypeWorker {
    type Ou$0 // complete_verbose_with_source("type trait member prefix")
}

struct ConstWorker;
impl Service for ConstWorker {
    const LI$0 // complete_verbose_with_source("const trait member prefix")
}
"#,
        expect![[r#"
            // function trait member prefix
            fn required
              detail: required trait member: fn required(&self) -> Self::Output
              filter: fn required
              sort: 00-required|required|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(0) })
              replace: 161..166
              snippet: fn required(&self) -> Self::Output {
                ${1:todo!()}
            }

            // type trait member prefix
            type_alias Output
              detail: required trait member: type Output = ()
              filter: type Output
              sort: 00-required|Output|04|00|Declaration(DeclarationRef { kind: "item", item: TypeAlias(TypeAliasRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: TypeAliasId(0) }) })
              replace: 223..230
              snippet: type Output = ${1:()};

            // const trait member prefix
            const LIMIT
              detail: required trait member: const LIMIT: usize
              filter: const LIMIT
              sort: 00-required|LIMIT|05|00|Declaration(DeclarationRef { kind: "item", item: Const(ConstRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: ConstId(0) }) })
              replace: 289..297
              snippet: const LIMIT: usize = ${1:todo!()};
        "#]],
    );
}

#[test]
fn completes_module_scope_macro_invocations_only() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_module_macro_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! local_item {
    () => { struct Generated; };
}

pub fn ordinary_function() {}
pub struct OrdinaryType;

mod tools {
    macro_rules! nested_item {
        () => { struct NestedGenerated; };
    }
    pub(crate) use nested_item;
}

local_i$0!(); // complete_verbose_with_source("unqualified module macro")
tools::nested_i$0!(); // complete_verbose_with_source("qualified module macro")
"#,
        expect![[r#"
            // unqualified module macro
            macro local_item
              detail: macro local_item
              sort: local_item|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(0) } })
              replace: 245..252

            // qualified module macro
            macro nested_item
              detail: macro nested_item
              sort: nested_item|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(3) } })
              replace: 264..272
        "#]],
    );
}

#[test]
fn completes_conventional_out_of_line_module_names() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_module_declaration_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod already;
mod pars$0; // complete_verbose_with_source("root module declaration")
#[path = "custom.rs"]
mod cust$0; // complete_verbose_with_source("path-attributed module declaration")

mod inner {
    mod chi$0; // complete_verbose_with_source("inline module declaration")
}

//- /src/already.rs
pub struct Already;

//- /src/parser.rs
pub struct FlatParser;

//- /src/parser/mod.rs
pub struct NestedParser;

//- /src/type.rs
pub struct KeywordModule;

//- /src/invalid-name.rs
pub struct InvalidName;

//- /src/inner/child.rs
pub struct Child;
"#,
        expect![[r#"
            // root module declaration
            module parser
              detail: mod parser
              sort: parser|03|00|Synthetic(ModuleDeclaration)
              replace: 17..21
            module r#type
              detail: mod r#type
              sort: r#type|03|00|Synthetic(ModuleDeclaration)
              replace: 17..21

            <none> // path-attributed module declaration

            // inline module declaration
            module child
              detail: mod child
              sort: child|03|00|Synthetic(ModuleDeclaration)
              replace: 76..79
        "#]],
    );
}

#[test]
fn completes_modules_from_semantic_file_contexts() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "semantic_module_context_completions"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[[bin]]
name = "semantic-context-tool"
path = "src/tool.rs"

//- /src/lib.rs
#[path = "custom_imp.rs"]
mod implementation;

//- /src/custom_imp.rs
mod bes$0; // complete_verbose_with_source("path-selected definition file").matching("beside")

//- /src/beside.rs
pub struct Beside;

//- /src/custom_imp/below.rs
pub struct Below;

//- /src/tool.rs
mod roo$0; // complete_verbose_with_source("custom target root").in_bin("semantic_module_context_completions").matching("root_child")

//- /src/root_child.rs
pub struct RootChild;

//- /src/tool/wrong_child.rs
pub struct WrongChild;
"#,
        expect![[r#"
            // path-selected definition file
            module beside
              detail: mod beside
              sort: beside|03|00|Synthetic(ModuleDeclaration)
              replace: 4..7

            // custom target root
            module root_child
              detail: mod root_child
              sort: root_child|03|00|Synthetic(ModuleDeclaration)
              replace: 4..7
        "#]],
    );
}

#[test]
fn completes_qualified_and_unqualified_body_macros() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_macro_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
/// Builds a value through an unqualified declarative macro.
macro_rules! make_value {
    () => { 1u8 };
}

/// Builds a value through a crate-root macro.
#[macro_export]
macro_rules! exported_value {
    () => { 1u8 };
}

pub fn make_function() -> u8 {
    0
}

pub fn exported_function() -> u8 {
    0
}

pub fn use_it() {
    let _plain = make$0; // complete_verbose("macro from value prefix")
    let _qualified = crate::exported$0; // complete_verbose("qualified macro from value prefix")
}
"#,
        expect![[r#"
            // macro from value prefix
            fn exported_function
              detail: pub fn exported_function() -> u8
              sort: 01-module|exported_function|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(1) })
              replace: 343..347
              snippet: exported_function()$0
            macro exported_value
              detail: macro exported_value
              sort: 01-module|exported_value|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 343..347
              snippet: exported_value!($0)
            fn make_function
              detail: pub fn make_function() -> u8
              sort: 01-module|make_function|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(0) })
              replace: 343..347
              snippet: make_function()$0
            macro make_value
              detail: macro make_value
              sort: 01-module|make_value|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(0) } })
              replace: 343..347
              snippet: make_value!($0)
            fn use_it
              detail: pub fn use_it()
              sort: 01-module|use_it|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(2) })
              replace: 343..347
              snippet: use_it()$0

            // qualified macro from value prefix
            fn exported_function
              detail: pub fn exported_function() -> u8
              sort: exported_function|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(1) })
              replace: 377..385
              snippet: exported_function()$0
            macro exported_value
              detail: macro exported_value
              sort: exported_value|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 377..385
              snippet: exported_value!($0)
            fn make_function
              detail: pub fn make_function() -> u8
              sort: make_function|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(0) })
              replace: 377..385
              snippet: make_function()$0
            macro make_value
              detail: macro make_value
              sort: make_value|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(0) } })
              replace: 377..385
              snippet: make_value!($0)
            fn use_it
              detail: pub fn use_it()
              sort: use_it|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(2) })
              replace: 377..385
              snippet: use_it()$0
        "#]],
    );
}

#[test]
fn proc_macro_completions_keep_export_identity_and_hide_implementation_items() {
    check_analysis(
        r#"
//- /Cargo.toml
[workspace]
members = ["macros", "app"]
resolver = "3"

//- /macros/Cargo.toml
[package]
name = "completion_proc_macros"
version = "0.1.0"
edition = "2024"

[lib]
proc-macro = true

//- /macros/src/lib.rs
extern crate proc_macro;

#[proc_macro]
pub fn emit(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    input
}

#[proc_macro_attribute]
pub fn traced(
    _attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    item
}

#[proc_macro_derive(Stored)]
pub fn stored(_item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    proc_macro::TokenStream::new()
}

pub fn leaked_value() {}
pub struct LeakedType;

//- /app/Cargo.toml
[package]
name = "completion_proc_macro_app"
version = "0.1.0"
edition = "2024"

[dependencies]
completion_proc_macros = { path = "../macros" }

//- /app/src/lib.rs
pub mod already_invoked;
pub mod attribute;
pub mod derive;
pub mod implementation;
pub mod leaked;
pub mod qualified;
pub mod unqualified;

//- /app/src/unqualified.rs
use completion_proc_macros::emit;

fn use_macro() {
    let _ = em$0 // complete_verbose_with_source("unqualified function-like proc macro at EOF").in_lib("completion_proc_macro_app").matching("emit")

//- /app/src/already_invoked.rs
use completion_proc_macros::emit;

fn use_macro() {
    let _ = em$0!(); // complete_verbose_with_source("function-like proc macro with invocation already typed").in_lib("completion_proc_macro_app").matching("emit")
}

//- /app/src/qualified.rs
fn use_macro() {
    let _ = completion_proc_macros::em$0 // complete_verbose_with_source("qualified function-like proc macro at EOF").in_lib("completion_proc_macro_app").matching("emit")

//- /app/src/attribute.rs
#[completion_proc_macros::tra$0 // complete_verbose_with_source("attribute proc macro keeps macro detail").in_lib("completion_proc_macro_app").matching("traced")

//- /app/src/derive.rs
#[derive(completion_proc_macros::Sto$0 // complete_verbose_with_source("derive proc macro keeps export name and macro detail").in_lib("completion_proc_macro_app").matching("Stored")

//- /app/src/implementation.rs
fn cannot_call_implementation() {
    let _ = completion_proc_macros::sto$0 // complete_with_source("implementation function is hidden downstream").in_lib("completion_proc_macro_app").matching("stored")

//- /app/src/leaked.rs
fn cannot_name_other_exports() {
    let _ = completion_proc_macros::Lea$0 // complete_with_source("ordinary proc-macro crate exports are hidden downstream").in_lib("completion_proc_macro_app").matching("Leaked")
"#,
        expect![[r#"
            // unqualified function-like proc macro at EOF
            macro emit
              detail: macro emit
              sort: 01-module|emit|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 64..66
              snippet: emit!($0)

            // function-like proc macro with invocation already typed
            macro emit
              detail: macro emit
              sort: 01-module|emit|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 64..66

            // qualified function-like proc macro at EOF
            macro emit
              detail: macro emit
              sort: emit|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 53..55
              snippet: emit!($0)

            // attribute proc macro keeps macro detail
            macro traced
              detail: macro traced
              sort: traced|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(3) } })
              replace: 26..29

            // derive proc macro keeps export name and macro detail
            macro Stored
              detail: macro Stored
              sort: Stored|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(5) } })
              replace: 33..36

            <none> // implementation function is hidden downstream
            <none> // ordinary proc-macro crate exports are hidden downstream
        "#]],
    );
}
