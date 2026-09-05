use expect_test::expect;

use super::super::utils::{AnalysisQuery, check_analysis_queries};
#[test]
fn completes_specialized_strings_macro_fragments_and_postfix_transforms() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "specialized_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
const MODULE_CAPTURE: usize = 1;
static STATIC_CAPTURE: usize = 2;

extern "C-un$abi$" fn foreign();

macro_rules! capture {
    ($value: ex$fragment$) => { $value };
}

fn complete<const COUNT: usize>(local_capture: usize, condition: bool, value: i32) {
    let nested_capture = local_capture;
    let _ = format!("{loc$format_local$}", explicit_name = value);
    let _ = format!("{exp$format_named$}", explicit_name = value);
    let _ = format!("{MOD$format_const$}");
    let _ = env!("CARGO_MAN$environment$");
    let _ = option_env!("CARGO_BIN$option_environment$");
    let _ = write!("ordinary$write_destination$", "{nes$write_format$}");
    let _ = (value + 1).ma$postfix_match$;
    let _ = condition.i$postfix_if$;
    let _ = value.i$non_bool_if$;
}
"#,
        &[
            AnalysisQuery::complete_verbose_with_source("format local", "format_local")
                .matching("local_capture"),
            AnalysisQuery::complete_verbose_with_source("format named argument", "format_named")
                .matching("explicit_name"),
            AnalysisQuery::complete_with_source("format module const", "format_const")
                .matching("MODULE_CAPTURE"),
            AnalysisQuery::complete_verbose_with_source("Cargo environment", "environment")
                .matching("CARGO_MANIFEST_DIR"),
            AnalysisQuery::complete_with_source("Cargo option environment", "option_environment")
                .matching("CARGO_BIN_NAME"),
            AnalysisQuery::complete_with_source(
                "write destination is ordinary",
                "write_destination",
            )
            .matching("nested_capture"),
            AnalysisQuery::complete_with_source("write format", "write_format")
                .matching("nested_capture"),
            AnalysisQuery::complete_verbose_with_source("extern ABI", "abi").matching("C-unwind"),
            AnalysisQuery::complete_verbose_with_source("macro fragment", "fragment")
                .matching("expr"),
            AnalysisQuery::complete_verbose_with_source("postfix match", "postfix_match")
                .matching("match"),
            AnalysisQuery::complete_verbose_with_source("boolean postfix", "postfix_if")
                .matching("if"),
            AnalysisQuery::complete_with_source("non-boolean postfix", "non_bool_if")
                .matching("if"),
        ],
        expect![[r#"
            format local
            - variable local_capture
              detail: format capture local_capture
              sort: 00-body:0001|local_capture|07|00|Declaration(DeclarationRef { kind: "binding", body: FunctionBodyRef { crate_ref: CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }, body: BodyId(0) }, binding: BindingId(0) })
              replace: 302..305

            format named argument
            - variable explicit_name
              detail: format argument explicit_name
              sort: 00-specialized:07:explicit_name
              replace: 355..358

            format module const
            - const MODULE_CAPTURE

            Cargo environment
            - value CARGO_MANIFEST_DIR
              detail: directory containing this package manifest
              sort: 00-specialized:08:CARGO_MANIFEST_DIR
              replace: 434..443

            Cargo option environment
            - value CARGO_BIN_NAME

            write destination is ordinary
            - <none>

            write format
            - variable nested_capture

            extern ABI
            - value C-unwind
              detail: extern ABI C-unwind
              sort: 00-specialized:08:C-unwind
              replace: 76..80

            macro fragment
            - value expr
              detail: macro fragment expr
              sort: 00-specialized:08:expr
              replace: 133..135
            - value expr_2021
              detail: macro fragment expr_2021
              sort: 00-specialized:08:expr_2021
              replace: 133..135

            postfix match
            - postfix match
              detail: postfix match expr
              sort: 00-specialized:09:match
              replace: 538..552
              snippet: match (value + 1) {
                ${1:_} => { $0 },
            }

            boolean postfix
            - postfix if
              detail: postfix if expr
              sort: 00-specialized:09:if
              replace: 566..577
              snippet: if condition {
                $0
            }

            non-boolean postfix
            - <none>
        "#]],
    );
}

#[test]
fn completes_attributes_lifetimes_labels_visibility_extern_and_const_sites() {
    check_analysis_queries(
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

#[der$attribute_path$]
struct AttributePath;

#[macros::tra$attribute_macro$]
struct AttributeMacro;

#[derive(Debug, Cl$derive_builtin$)]
struct BuiltinDerive;

#[derive(macros::Sto$derive_macro$)]
struct MacroDerive;

#[allow(dead$lint$)]
#[repr(tra$repr$)]
#[cfg(tar$cfg_key$)]
#[cfg(feature = "serde-$cfg_feature$")]
#[diagnostic::on_unimplemented(mes$diagnostic$)]
#[deprecated(si$compatibility$)]
struct AttributeInputs;

pub mod outer {
    pub mod inner {
        pub(in crate::outer::in$visibility_module$) struct Restricted;
    }
}

extern crate dep$extern_crate$;

struct Buffer<const N: usize = LIM$const_default$>([u8; N]);
struct Array([u8; LIM$array_length$]);

fn signature_lifetime<'outer>(value: &'outer u8) -> &'out$lifetime$ u8 {
    value
}

fn const_body<const N: usize>() {
    let _: [u8; N$const_param$];
    let _: Buffer<{ LIM$const_arg$ }>;

    'outer: loop {
        'inner: loop {
            break 'inn$label$;
        }
    }
}
"#,
        &[
            AnalysisQuery::complete_with_source("attribute path", "attribute_path")
                .in_lib("app")
                .matching("derive"),
            AnalysisQuery::complete_with_source("attribute proc macro", "attribute_macro")
                .in_lib("app")
                .matching("traced"),
            AnalysisQuery::complete_with_source("builtin derive", "derive_builtin")
                .in_lib("app")
                .matching("Clone"),
            AnalysisQuery::complete_with_source("derive proc macro", "derive_macro")
                .in_lib("app")
                .matching("Stored"),
            AnalysisQuery::complete_with_source("lint input", "lint")
                .in_lib("app")
                .matching("dead_code"),
            AnalysisQuery::complete_with_source("repr input", "repr")
                .in_lib("app")
                .matching("transparent"),
            AnalysisQuery::complete_with_source("cfg key", "cfg_key")
                .in_lib("app")
                .matching("target_arch"),
            AnalysisQuery::complete_verbose_with_source("cfg Cargo feature", "cfg_feature")
                .in_lib("app")
                .matching("serde-support"),
            AnalysisQuery::complete_with_source("diagnostic input", "diagnostic")
                .in_lib("app")
                .matching("message"),
            AnalysisQuery::complete_with_source("compatibility input", "compatibility")
                .in_lib("app")
                .matching("since"),
            AnalysisQuery::complete_with_source("visibility module", "visibility_module")
                .in_lib("app")
                .matching("inner"),
            AnalysisQuery::complete_with_source("extern crate", "extern_crate")
                .in_lib("app")
                .matching("dependency"),
            AnalysisQuery::complete_with_source("const default", "const_default")
                .in_lib("app")
                .matching("LIMIT"),
            AnalysisQuery::complete_with_source("array length", "array_length")
                .in_lib("app")
                .matching("LIMIT"),
            AnalysisQuery::complete_with_source("lifetime", "lifetime")
                .in_lib("app")
                .matching("'outer"),
            AnalysisQuery::complete_with_source("const parameter", "const_param")
                .in_lib("app")
                .matching("N"),
            AnalysisQuery::complete_with_source("braced const argument", "const_arg")
                .in_lib("app")
                .matching("LIMIT"),
            AnalysisQuery::complete_with_source("loop label", "label")
                .in_lib("app")
                .matching("'inner"),
        ],
        expect![[r#"
            attribute path
            - attribute derive

            attribute proc macro
            - macro traced

            builtin derive
            - value Clone

            derive proc macro
            - macro Stored

            lint input
            - value dead_code

            repr input
            - value transparent

            cfg key
            - value target_arch

            cfg Cargo feature
            - value serde-support
              detail: attribute input serde-support
              sort: 00-specialized:08:serde-support
              replace: 272..278

            diagnostic input
            - value message

            compatibility input
            - value since

            visibility module
            - module inner

            extern crate
            - module dependency

            const default
            - const LIMIT

            array length
            - const LIMIT

            lifetime
            - lifetime 'outer

            const parameter
            - const N

            braced const argument
            - const LIMIT

            loop label
            - label 'inner
        "#]],
    );
}

#[test]
fn completes_missing_trait_impl_members_with_instantiated_stubs() {
    check_analysis_queries(
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

    req$missing_members$
}
"#,
        &[AnalysisQuery::complete_verbose_with_source(
            "missing trait members",
            "missing_members",
        )],
        expect![[r#"
            missing trait members
            - const REQUIRED
              detail: required trait member: const REQUIRED: u8
              sort: 00-required|REQUIRED|05|00|Declaration(DeclarationRef { kind: "item", item: Const(ConstRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: ConstId(0) }) })
              replace: 411..414
              snippet: const REQUIRED: u8 = ${1:todo!()};
            - fn required
              detail: required trait member: fn required(&self, value: u8) -> Self::Output
              docs: Handle one request.
              sort: 00-required|required|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(0) })
              replace: 411..414
              snippet: fn required(&self, value: u8) -> Self::Output {
                ${1:todo!()}
            }
            - const DEFAULTED
              detail: default trait member: const DEFAULTED: u8
              sort: 01-default|DEFAULTED|05|00|Declaration(DeclarationRef { kind: "item", item: Const(ConstRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: ConstId(1) }) })
              replace: 411..414
              snippet: const DEFAULTED: u8 = ${1:todo!()};
            - type_alias Defaulted
              detail: default trait member: type Defaulted = u8
              sort: 01-default|Defaulted|04|00|Declaration(DeclarationRef { kind: "item", item: TypeAlias(TypeAliasRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: TypeAliasId(1) }) })
              replace: 411..414
              snippet: type Defaulted = ${1:u8};
        "#]],
    );
}

#[test]
fn trait_impl_member_introducers_filter_and_replace_the_whole_prefix() {
    check_analysis_queries(
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
    fn re$function$
}

struct TypeWorker;
impl Service for TypeWorker {
    type Ou$type_alias$
}

struct ConstWorker;
impl Service for ConstWorker {
    const LI$const$
}
"#,
        &[
            AnalysisQuery::complete_verbose_with_source("function trait member prefix", "function"),
            AnalysisQuery::complete_verbose_with_source("type trait member prefix", "type_alias"),
            AnalysisQuery::complete_verbose_with_source("const trait member prefix", "const"),
        ],
        expect![[r#"
            function trait member prefix
            - fn required
              detail: required trait member: fn required(&self) -> Self::Output
              filter: fn required
              sort: 00-required|required|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(0) })
              replace: 161..166
              snippet: fn required(&self) -> Self::Output {
                ${1:todo!()}
            }

            type trait member prefix
            - type_alias Output
              detail: required trait member: type Output = ()
              filter: type Output
              sort: 00-required|Output|04|00|Declaration(DeclarationRef { kind: "item", item: TypeAlias(TypeAliasRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: TypeAliasId(0) }) })
              replace: 223..230
              snippet: type Output = ${1:()};

            const trait member prefix
            - const LIMIT
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
    check_analysis_queries(
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

local_i$unqualified_macro$!();
tools::nested_i$qualified_macro$!();
"#,
        &[
            AnalysisQuery::complete_verbose_with_source(
                "unqualified module macro",
                "unqualified_macro",
            ),
            AnalysisQuery::complete_verbose_with_source(
                "qualified module macro",
                "qualified_macro",
            ),
        ],
        expect![[r#"
            unqualified module macro
            - macro local_item
              detail: macro local_item
              sort: local_item|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(0) } })
              replace: 245..252

            qualified module macro
            - macro nested_item
              detail: macro nested_item
              sort: nested_item|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(3) } })
              replace: 264..272
        "#]],
    );
}

#[test]
fn completes_conventional_out_of_line_module_names() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_module_declaration_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod already;
mod pars$root_module$;
#[path = "custom.rs"]
mod cust$path_module$;

mod inner {
    mod chi$inline_module$;
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
        &[
            AnalysisQuery::complete_verbose_with_source("root module declaration", "root_module"),
            AnalysisQuery::complete_verbose_with_source(
                "path-attributed module declaration",
                "path_module",
            ),
            AnalysisQuery::complete_verbose_with_source(
                "inline module declaration",
                "inline_module",
            ),
        ],
        expect![[r#"
            root module declaration
            - module parser
              detail: mod parser
              sort: parser|03|00|Synthetic(ModuleDeclaration)
              replace: 17..21
            - module r#type
              detail: mod r#type
              sort: r#type|03|00|Synthetic(ModuleDeclaration)
              replace: 17..21

            path-attributed module declaration
            - <none>

            inline module declaration
            - module child
              detail: mod child
              sort: child|03|00|Synthetic(ModuleDeclaration)
              replace: 76..79
        "#]],
    );
}

#[test]
fn completes_modules_from_semantic_file_contexts() {
    check_analysis_queries(
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
mod bes$path_selected_file$;

//- /src/beside.rs
pub struct Beside;

//- /src/custom_imp/below.rs
pub struct Below;

//- /src/tool.rs
mod roo$custom_target_root$;

//- /src/root_child.rs
pub struct RootChild;

//- /src/tool/wrong_child.rs
pub struct WrongChild;
"#,
        &[
            AnalysisQuery::complete_verbose_with_source(
                "path-selected definition file",
                "path_selected_file",
            )
            .matching("beside"),
            AnalysisQuery::complete_verbose_with_source("custom target root", "custom_target_root")
                .in_bin("semantic_module_context_completions")
                .matching("root_child"),
        ],
        expect![[r#"
            path-selected definition file
            - module beside
              detail: mod beside
              sort: beside|03|00|Synthetic(ModuleDeclaration)
              replace: 4..7

            custom target root
            - module root_child
              detail: mod root_child
              sort: root_child|03|00|Synthetic(ModuleDeclaration)
              replace: 4..7
        "#]],
    );
}

#[test]
fn completes_qualified_and_unqualified_body_macros() {
    check_analysis_queries(
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
    let _plain = make$plain_macro_prefix$;
    let _qualified = crate::exported$qualified_prefix$;
}
"#,
        &[
            AnalysisQuery::complete_verbose("macro from value prefix", "plain_macro_prefix"),
            AnalysisQuery::complete_verbose(
                "qualified macro from value prefix",
                "qualified_prefix",
            ),
        ],
        expect![[r#"
            macro from value prefix
            - fn exported_function
              detail: pub fn exported_function() -> u8
              sort: 01-module|exported_function|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(1) })
              replace: 343..347
              snippet: exported_function()$0
            - macro exported_value
              detail: macro exported_value
              sort: 01-module|exported_value|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 343..347
              snippet: exported_value!($0)
            - fn make_function
              detail: pub fn make_function() -> u8
              sort: 01-module|make_function|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(0) })
              replace: 343..347
              snippet: make_function()$0
            - macro make_value
              detail: macro make_value
              sort: 01-module|make_value|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(0) } })
              replace: 343..347
              snippet: make_value!($0)
            - fn use_it
              detail: pub fn use_it()
              sort: 01-module|use_it|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(2) })
              replace: 343..347
              snippet: use_it()$0

            qualified macro from value prefix
            - fn exported_function
              detail: pub fn exported_function() -> u8
              sort: exported_function|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(1) })
              replace: 377..385
              snippet: exported_function()$0
            - macro exported_value
              detail: macro exported_value
              sort: exported_value|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 377..385
              snippet: exported_value!($0)
            - fn make_function
              detail: pub fn make_function() -> u8
              sort: make_function|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(0) })
              replace: 377..385
              snippet: make_function()$0
            - macro make_value
              detail: macro make_value
              sort: make_value|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(0) } })
              replace: 377..385
              snippet: make_value!($0)
            - fn use_it
              detail: pub fn use_it()
              sort: use_it|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(2) })
              replace: 377..385
              snippet: use_it()$0
        "#]],
    );
}

#[test]
fn proc_macro_completions_keep_export_identity_and_hide_implementation_items() {
    check_analysis_queries(
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
    let _ = em$unqualified_proc_macro_eof$

//- /app/src/already_invoked.rs
use completion_proc_macros::emit;

fn use_macro() {
    let _ = em$already_invoked_proc_macro$!();
}

//- /app/src/qualified.rs
fn use_macro() {
    let _ = completion_proc_macros::em$qualified_proc_macro_eof$

//- /app/src/attribute.rs
#[completion_proc_macros::tra$attribute_proc_macro_eof$

//- /app/src/derive.rs
#[derive(completion_proc_macros::Sto$derive_proc_macro_eof$

//- /app/src/implementation.rs
fn cannot_call_implementation() {
    let _ = completion_proc_macros::sto$proc_macro_implementation_eof$

//- /app/src/leaked.rs
fn cannot_name_other_exports() {
    let _ = completion_proc_macros::Lea$proc_macro_leak_eof$
"#,
        &[
            AnalysisQuery::complete_verbose_with_source(
                "unqualified function-like proc macro at EOF",
                "unqualified_proc_macro_eof",
            )
            .in_lib("completion_proc_macro_app")
            .matching("emit"),
            AnalysisQuery::complete_verbose_with_source(
                "function-like proc macro with invocation already typed",
                "already_invoked_proc_macro",
            )
            .in_lib("completion_proc_macro_app")
            .matching("emit"),
            AnalysisQuery::complete_verbose_with_source(
                "qualified function-like proc macro at EOF",
                "qualified_proc_macro_eof",
            )
            .in_lib("completion_proc_macro_app")
            .matching("emit"),
            AnalysisQuery::complete_verbose_with_source(
                "attribute proc macro keeps macro detail",
                "attribute_proc_macro_eof",
            )
            .in_lib("completion_proc_macro_app")
            .matching("traced"),
            AnalysisQuery::complete_verbose_with_source(
                "derive proc macro keeps export name and macro detail",
                "derive_proc_macro_eof",
            )
            .in_lib("completion_proc_macro_app")
            .matching("Stored"),
            AnalysisQuery::complete_with_source(
                "implementation function is hidden downstream",
                "proc_macro_implementation_eof",
            )
            .in_lib("completion_proc_macro_app")
            .matching("stored"),
            AnalysisQuery::complete_with_source(
                "ordinary proc-macro crate exports are hidden downstream",
                "proc_macro_leak_eof",
            )
            .in_lib("completion_proc_macro_app")
            .matching("Leaked"),
        ],
        expect![[r#"
            unqualified function-like proc macro at EOF
            - macro emit
              detail: macro emit
              sort: 01-module|emit|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 64..66
              snippet: emit!($0)

            function-like proc macro with invocation already typed
            - macro emit
              detail: macro emit
              sort: 01-module|emit|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 64..66

            qualified function-like proc macro at EOF
            - macro emit
              detail: macro emit
              sort: emit|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 53..55
              snippet: emit!($0)

            attribute proc macro keeps macro detail
            - macro traced
              detail: macro traced
              sort: traced|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(3) } })
              replace: 26..29

            derive proc macro keeps export name and macro detail
            - macro Stored
              detail: macro Stored
              sort: Stored|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(5) } })
              replace: 33..36

            implementation function is hidden downstream
            - <none>

            ordinary proc-macro crate exports are hidden downstream
            - <none>
        "#]],
    );
}
