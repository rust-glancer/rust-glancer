use expect_test::expect;

use super::utils::{
    AnalysisQuery, check_analysis, check_analysis_queries, check_analysis_with_fake_sysroot,
};

#[test]
fn resolves_body_references_to_definition_targets() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_goto_definition
pub struct User;

pub fn helper() -> User {
    User
}

pub fn use_it() {
    let local: User = helper();
    let _again: User = loc$0al; // goto("goto local")
    let _made: User = hel$0per(); // goto("goto item")
}
"#,
        expect![[r#"
            local local @ 8:9-8:14 // goto local
            fn helper @ 3:8-3:14 // goto item
        "#]],
    );
}

#[test]
fn resolves_binding_declarations_to_themselves() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_binding_goto
pub struct User;

pub fn helper() -> User {
    User
}

pub fn use_it() {
    let loc$0al: User = helper(); // goto("goto declaration binding")
}
"#,
        expect![[r#"
            local local @ 8:9-8:14 // goto declaration binding
        "#]],
    );
}

#[test]
fn lands_on_declaration_names_after_doc_comments() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_doc_comment_goto
/// User docs.
pub struct User;

/// Builds a user.
pub fn make() -> User {
    User
}

pub fn use_it() {
    let _user: Us$0er = ma$1ke(); // goto("goto documented type"), goto("goto documented function")
}
"#,
        expect![[r#"
            struct User @ 2:12-2:16 // goto documented type
            fn make @ 5:8-5:12 // goto documented function
        "#]],
    );
}

#[test]
fn resolves_body_macro_calls_to_macro_definitions() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_macro_call_goto
pub struct Text;

pub fn helper(value: u64) -> Text { Text }

/// Builds text from a value.
macro_rules! make_text {
    ($value:expr) => { helper($value) };
}

pub fn use_it(input: u64) {
    let _text = make_$0text!(input); // goto("goto body macro call")
}
"#,
        expect![[r#"
            macro make_text @ 6:14-6:23 // goto body macro call
        "#]],
    );
}

#[test]
fn resolves_proc_macro_calls_to_their_implementation_source() {
    check_analysis(
        r#"
//- /macros/src/lib.rs crate:goto_proc_macros root:macros kind:proc-macro
extern crate proc_macro;

/// Emits its input unchanged.
#[proc_macro]
pub fn emit(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    input
}

//- /app/src/lib.rs crate:goto_proc_macro_app root:app deps:goto_proc_macros
use goto_proc_macros::emit;

pub fn use_it() {
    em$0it!(); // goto("goto proc macro implementation").in_lib("goto_proc_macro_app")
}
"#,
        expect![[r#"
            macro emit @ 5:8-5:12 // goto proc macro implementation
        "#]],
    );
}

#[test]
fn named_module_targets_select_module_name() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_named_module_goto
mod api {
    pub struct User;
}

pub fn use_it(_: a$0pi::User) {} // goto("goto module")
"#,
        expect![[r#"
            module api @ 1:5-1:8 // goto module
        "#]],
    );
}

#[test]
fn resolves_field_accesses_to_field_declarations() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_field_goto
pub struct Profile;

pub struct User {
    pub profile: Profile,
}

pub fn use_it(user: User) {
    let _profile: Profile = user.pro$0file; // goto("goto field")
}
"#,
        expect![[r#"
            field profile @ 4:9-4:16 // goto field
        "#]],
    );
}

#[test]
fn resolves_body_local_field_accesses_to_field_declarations() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_local_field_goto
pub struct GlobalId;

pub fn use_it() {
    struct User {
        local_id: GlobalId,
    }

    let user: User;
    let _id: GlobalId = user.loc$0al_id; // goto("goto body-local field")
}
"#,
        expect![[r#"
            field local_id @ 5:9-5:17 // goto body-local field
        "#]],
    );
}

#[test]
fn resolves_body_local_method_calls_to_method_declarations() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_local_method_goto
pub struct GlobalId;

pub fn use_it() {
    struct User;

    impl User {
        fn id(&self) -> GlobalId {
            missing()
        }
    }

    let user: User;
    let _id: GlobalId = user.i$0d(); // goto("goto body-local method")
}
"#,
        expect![[r#"
            fn id @ 7:12-7:14 // goto body-local method
        "#]],
    );
}

#[test]
fn resolves_lifetime_parameterized_receiver_method_calls() {
    check_analysis_queries(
        r#"
//- /src/lib.rs crate:analysis_lifetime_receiver_method_goto
fn missing<T>() -> T {
    loop {}
}

pub struct Module;
pub struct DebugStruct;

pub struct TargetScopeCollector<'db> {
    module: &'db Module,
}

impl<'db> TargetScopeCollector<'db> {
    fn all$goto_alloc_decl$oc_module(&mut self) -> Module {
        missing()
    }

    fn col$goto_collect_decl$lect_items(&mut self) -> Module {
        self.all$goto_self_alloc$oc_module()
    }

    fn collect(&mut self) -> Module {
        self.col$goto_self_collect$lect_items()
    }
}

pub mod fmt {
    pub struct Formatter<'a> {
        marker: &'a (),
    }
}

impl<'a> fmt::Formatter<'a> {
    fn de$goto_debug_decl$bug_struct(&mut self) -> crate::DebugStruct {
        crate::missing()
    }
}

pub fn use_it<'db>(
    collector: &mut TargetScopeCollector<'db>,
    formatter: &mut fmt::Formatter<'_>,
) {
    let _items = collector.col$goto_collect$lect_items();
    let _debug = formatter.de$goto_debug$bug_struct();
}
"#,
        &[
            AnalysisQuery::goto("goto self method in lifetime impl", "goto_self_alloc"),
            AnalysisQuery::goto(
                "goto self sibling method in lifetime impl",
                "goto_self_collect",
            ),
            AnalysisQuery::goto("goto external method on lifetime receiver", "goto_collect"),
            AnalysisQuery::goto(
                "goto method on Formatter placeholder lifetime",
                "goto_debug",
            ),
        ],
        expect![[r#"
            fn alloc_module @ 13:8-13:20 // goto self method in lifetime impl
            fn collect_items @ 17:8-17:21 // goto self sibling method in lifetime impl
            fn collect_items @ 17:8-17:21 // goto external method on lifetime receiver
            fn debug_struct @ 33:8-33:20 // goto method on Formatter placeholder lifetime
        "#]],
    );
}

#[test]
fn resolves_associated_functions_and_enum_variants_in_body_paths() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_associated_path_goto
pub struct Widget;

impl Widget {
    pub fn create() -> Self {
        Widget
    }
}

pub enum Action {
    Configure(Widget),
    Project { widget: Widget },
    Quit,
}

pub const EXACT: i32 = 5;
pub const shadow: i32 = 7;

pub mod limits {
    pub const MIN: i32 = 0;
    pub const MAX: i32 = 10;
}

pub fn use_it(action: Action, value: i32) {
    let widget = Wi$0dget::cre$1ate(); // goto("goto associated type prefix"), goto("goto associated function")
    let _action = A$0ction::Con$1figure(Widget::create()); // goto("goto enum expression prefix"), goto("goto expression variant")

    match action {
        A$0ction::Con$1figure(widget) => widget, // goto("goto tuple pattern enum prefix"), goto("goto tuple pattern variant")
        Action::Pro$0ject { widget } => widget, // goto("goto record pattern variant")
        Action::Quit => Widget,
    };

    match value {
        limits::MI$0N..=limits::MA$1X => Widget, // goto("goto range pattern start"), goto("goto range pattern end")
        const { EX$0ACT } => Widget, // goto("goto const block pattern path")
        _ => Widget,
    };

    match (value, value) {
        (shadow, const { sha$0dow }) => Widget, // goto("goto const block shadowed path")
        _ => Widget,
    };
}
"#,
        expect![[r#"
            struct Widget @ 1:12-1:18 // goto associated type prefix
            fn create @ 4:12-4:18 // goto associated function
            enum Action @ 9:10-9:16 // goto enum expression prefix
            variant Configure @ 10:5-10:14 // goto expression variant
            enum Action @ 9:10-9:16 // goto tuple pattern enum prefix
            variant Configure @ 10:5-10:14 // goto tuple pattern variant
            variant Project @ 11:5-11:12 // goto record pattern variant
            const MIN @ 19:15-19:18 // goto range pattern start
            const MAX @ 20:15-20:18 // goto range pattern end
            const EXACT @ 15:11-15:16 // goto const block pattern path
            const shadow @ 16:11-16:17 // goto const block shadowed path
        "#]],
    );
}

#[test]
fn resolves_unkeyed_inherent_associated_paths() {
    check_analysis(
        r#"
//- /runtime/src/lib.rs crate:runtime root:runtime
impl u32 {
    pub const MAX: u32 = 0;
    pub fn from_be_bytes(_bytes: [u8; 4]) -> Self { 0 }
}

//- /app/src/lib.rs crate:app root:app deps:runtime
pub fn use_it(bytes: [u8; 4]) {
    let _max = u32::M$0AX; // goto("goto primitive associated const").in_lib("app")
    let _value = u32::from_be_$0bytes(bytes); // goto("goto primitive associated function").in_lib("app")
}
"#,
        expect![[r#"
            const MAX @ 2:15-2:18 // goto primitive associated const
            fn from_be_bytes @ 3:12-3:25 // goto primitive associated function
        "#]],
    );
}

#[test]
fn resolves_crate_prefixes_inside_body_type_paths() {
    check_analysis(
        r#"
//- /crates/helper/src/lib.rs crate:helper root:crates/helper
pub struct Tool;

//- /crates/app/src/lib.rs crate:app root:crates/app deps:helper
pub fn use_it() {
    let _tool: hel$0per::Tool = todo!(); // goto("goto crate prefix").in_lib("app")
}
"#,
        expect![[r#"
            module crate @ <root> // goto crate prefix
        "#]],
    );
}

#[test]
fn resolves_tuple_field_accesses_to_tuple_field_declarations() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_tuple_field_goto
pub struct Left;
pub struct Right;

pub struct Pair(pub Left, pub Right);

pub fn use_it(pair: Pair) {
    let _left: Left = pair.$0$0; // goto("goto tuple field")
}
"#,
        expect![[r#"
            field #0 @ 4:17-4:25 // goto tuple field
        "#]],
    );
}

#[test]
fn resolves_direct_trait_method_calls_to_impl_methods() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_direct_trait_method_goto
pub struct User;

pub trait Identify {
    fn id(&self);
}

impl Identify for User {
    fn id(&self) {}
}

pub fn use_it(user: User) {
    user.i$0d(); // goto("goto direct trait method")
}
"#,
        expect![[r#"
            fn id @ 8:8-8:10 // goto direct trait method
        "#]],
    );
}

#[test]
fn follows_the_selected_self_type_for_trait_method_definitions() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_selected_receiver_goto
pub trait Record {
    fn record(&self);
}
pub struct Predicate;
impl Record for Predicate {
    fn record(&self) {}
}
impl<T> Record for &T {
    fn record(&self) {}
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
    fn record(&self) {}
}
pub struct DerefOnly;
impl Deref for DerefOnly {
    type Target = Predicate;
    fn deref(&self) -> &Predicate { missing() }
}

pub fn match_binding(gate: &Gate) {
    match gate {
        Gate::Direct(predicate) => predicate.rec$0ord(), // goto("borrowed match binding")
    }
}
pub fn double_reference(value: &&Predicate) { value.rec$0ord(); } // goto("double reference selects the reference impl")
pub fn reference_only(value: &ReferenceOnly) { value.rec$0ord(); } // goto("reference-only implementation")
pub fn own_wrapper(value: Wrapper) { value.rec$0ord(); } // goto("wrapper implementation before Deref")
pub fn through_deref(value: DerefOnly) { value.rec$0ord(); } // goto("implementation through Deref")
pub fn explicit_reference(value: &&Predicate) {
    <&Predicate as Record>::rec$0ord(value); // goto("explicit reference Self")
}
"#,
        expect![[r#"
            fn record @ 6:8-6:14 // borrowed match binding
            fn record @ 9:8-9:14 // double reference selects the reference impl
            fn record @ 9:8-9:14 // reference-only implementation
            fn record @ 26:8-26:14 // wrapper implementation before Deref
            fn record @ 6:8-6:14 // implementation through Deref
            fn record @ 9:8-9:14 // explicit reference Self
        "#]],
    );
}

#[test]
fn selects_trait_call_definitions_with_finalized_generic_arguments() {
    check_analysis_queries(
        r#"
//- /src/lib.rs crate:analysis_trait_call_definition_arguments
pub struct Source;

pub trait Convert<T> {
    fn convert<U>(&self, input: U) -> T;
}

impl Convert<u32> for Source {
    fn convert<U>(&self, input: U) -> u32 { 0 }
}

impl Convert<u16> for Source {
    fn convert<U>(&self, input: U) -> u16 { 0 }
}

pub fn use_it(source: Source) {
    let wide: u32 = source.con$wide$vert::<u8>(0u8);
    let narrow: u16 = source.con$narrow$vert::<u8>(0u8);
    let explicit = <Source as Convert<u16>>::con$qualified$vert::<u8>(&source, 0u8);
    let associated: u32 = Source::con$associated$vert::<u8>(&source, 0u8);
    let shared: &&Source = &&source;
    let borrowed: u16 = shared.con$borrowed$vert::<u8>(0u8);
}
"#,
        &[
            AnalysisQuery::goto("wide conversion", "wide"),
            AnalysisQuery::goto("narrow conversion", "narrow"),
            AnalysisQuery::goto("qualified conversion", "qualified"),
            AnalysisQuery::goto("associated conversion", "associated"),
            AnalysisQuery::goto("borrowed conversion", "borrowed"),
            AnalysisQuery::resolve("resolve selected call symbol", "wide"),
            AnalysisQuery::goto_impl("all implementations of the conversion method", "wide"),
        ],
        expect![[r#"
            fn convert @ 8:8-8:15 // wide conversion
            fn convert @ 12:8-12:15 // narrow conversion
            fn convert @ 12:8-12:15 // qualified conversion
            fn convert @ 8:8-8:15 // associated conversion
            fn convert @ 12:8-12:15 // borrowed conversion
            fn convert @ 8:8-8:15 // resolve selected call symbol

            // all implementations of the conversion method
            fn convert @ 8:8-8:15
            fn convert @ 12:8-12:15
        "#]],
    );
}

#[test]
fn navigates_blanket_impl_calls_and_preserves_trait_body_destinations() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_trait_call_definition_bodies
pub trait Named {
    fn name(&self) {}
}

pub struct Wrapper<T>(T);
pub struct User;
pub trait Allowed {}

impl<T: Allowed> Named for Wrapper<T> {
    fn name(&self) {}
}

impl Named for User {}

pub fn generic<T: Allowed>(wrapped: Wrapper<T>, user: User) {
    wrapped.na$0me(); // goto("generic receiver with a blanket impl")
    user.na$0me(); // goto("inherited default body")
    User::na$0me(&user); // goto("associated call to the default body")
}

pub fn abstract_call<T: Named>(value: T) {
    value.na$0me(); // goto("call through a generic bound")
}
"#,
        expect![[r#"
            fn name @ 10:8-10:12 // generic receiver with a blanket impl
            fn name @ 2:8-2:12 // inherited default body
            fn name @ 2:8-2:12 // associated call to the default body
            fn name @ 2:8-2:12 // call through a generic bound
        "#]],
    );
}

#[test]
fn navigates_trait_calls_to_body_local_impl_methods() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_trait_call_definition_local
pub trait Named {
    fn name(&self);
}

pub fn use_it() {
    struct User;
    impl Named for User {
        fn name(&self) {}
    }
    let user = User;
    user.na$0me(); // goto("body-local trait implementation")
    User::na$0me(&user); // goto("body-local associated call")
}
"#,
        expect![[r#"
            fn name @ 8:12-8:16 // body-local trait implementation
            fn name @ 8:12-8:16 // body-local associated call
        "#]],
    );
}

#[test]
fn navigates_trait_calls_through_deref_to_cross_crate_primitive_impls() {
    check_analysis(
        r#"
//- /runtime/src/lib.rs crate:runtime root:runtime
pub trait Named {
    fn name(&self);
}

impl Named for u8 {
    fn name(&self) {}
}

#[lang = "deref"]
pub trait Deref {
    #[lang = "deref_target"]
    type Target: ?Sized;
    fn deref(&self) -> &Self::Target;
}

pub struct Wrapper;
impl Deref for Wrapper {
    type Target = u8;
    fn deref(&self) -> &u8 { missing() }
}

//- /app/src/lib.rs crate:app root:app deps:runtime
use runtime::{Named, Wrapper};

pub fn use_it(value: u8, wrapped: Wrapper) {
    value.na$0me(); // goto("primitive trait implementation").in_lib("app")
    wrapped.na$0me(); // goto("trait implementation after deref").in_lib("app")
}
"#,
        expect![[r#"
            fn name @ 6:8-6:12 // primitive trait implementation
            fn name @ 6:8-6:12 // trait implementation after deref
        "#]],
    );
}

#[test]
fn resolves_structural_and_macro_generated_method_calls() {
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
    /// Finds a substring.
    pub fn contains(&self, needle: &str) -> bool {
        missing()
    }
}

macro_rules! integer_methods {
    () => {
        /// Counts the set bits.
        pub fn generated_count(self) -> u32 {
            missing()
        }
    };
}

impl u32 {
    integer_methods!();
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

pub fn use_it(text: OwnedText, value: u32) {
    let _found = text.con$0tains("needle"); // goto("goto primitive method through Deref").in_lib("app")
    let _count = value.generated_$0count(); // goto("goto macro-generated primitive method").in_lib("app")
}
"#,
        expect![[r#"
            fn contains @ 11:12-11:20 // goto primitive method through Deref
            fn generated_count @ 26:5-26:24 // goto macro-generated primitive method
        "#]],
    );
}

#[test]
fn resolves_use_and_signature_paths_to_definitions() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_signature_goto
mod api {
    pub trait Named {}
    pub struct User;
}

use a$0pi::Us$1er; // goto("goto use module"), goto("goto use path")

impl api::Na$0med for Us$1er {} // goto("goto impl trait"), goto("goto impl self type")

pub fn make(user: Us$0er) -> Us$1er { // goto("goto parameter type"), goto("goto return type")
    user
}
"#,
        expect![[r#"
            module api @ 1:5-1:8 // goto use module
            struct User @ 3:16-3:20 // goto use path
            trait Named @ 2:15-2:20 // goto impl trait
            struct User @ 3:16-3:20 // goto impl self type
            struct User @ 3:16-3:20 // goto parameter type
            struct User @ 3:16-3:20 // goto return type
        "#]],
    );
}

#[test]
fn resolves_bin_root_paths_to_library_definitions() {
    check_analysis(
        r#"
//- /crates/dep/src/lib.rs crate:dep root:crates/dep
pub struct Thing;

//- /crates/app/src/lib.rs crate:app root:crates/app deps:dep
pub struct Api;

//- /crates/app/src/main.rs crate:app root:crates/app deps:dep kind:bin name:app-bin
fn main() {
    let _api: app::A$0pi = todo!(); // goto("goto bin root to library item").in_bin("app")
    let _thing: dep::Th$0ing = todo!(); // goto("goto bin root to dependency item").in_bin("app")
}
"#,
        expect![[r#"
            struct Api @ 1:12-1:15 // goto bin root to library item
            struct Thing @ 1:12-1:17 // goto bin root to dependency item
        "#]],
    );
}

#[test]
fn resolves_standard_prelude_signature_paths() {
    check_analysis_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:analysis_prelude_goto
pub fn use_it(_: Str$0ing) {} // goto("goto prelude type").in_lib("analysis_prelude_goto")
"#,
        expect![[r#"
            struct String @ 1:12-1:18 // goto prelude type
        "#]],
    );
}

#[test]
fn resolves_standard_prelude_signature_paths_shadowed_by_non_type_names() {
    check_analysis_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:analysis_prelude_shadow_goto
const String: u8 = 0;

pub fn value_shadow(_: Str$0ing) {} // goto("goto prelude type shadowed by value").in_lib("analysis_prelude_shadow_goto")

mod macro_shadow {
    macro_rules! String {
        () => {};
    }

    pub fn use_it(_: Str$0ing) {} // goto("goto prelude type shadowed by macro").in_lib("analysis_prelude_shadow_goto")
}
"#,
        expect![[r#"
            struct String @ 1:12-1:18 // goto prelude type shadowed by value
            struct String @ 1:12-1:18 // goto prelude type shadowed by macro
        "#]],
    );
}

#[test]
fn resolves_cursors_inside_out_of_line_nested_modules() {
    check_analysis(
        r#"
//- /src/lib.rs crate:nested_analysis
pub struct Api;
pub mod outer;

//- /src/outer.rs
pub mod inner;

//- /src/outer/inner.rs
pub fn use_it(_: crate::A$0pi) {} // goto("goto from nested file").in_lib("nested_analysis")
"#,
        expect![[r#"
            struct Api @ 1:12-1:15 // goto from nested file
        "#]],
    );
}

#[test]
fn resolves_field_declarations_and_field_type_paths() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_field_signature_goto
pub struct Profile;

pub struct User {
    pub pro$0file: Pro$1file, // goto("goto field declaration"), goto("goto field type")
}
"#,
        expect![[r#"
            field profile @ 4:9-4:16 // goto field declaration
            struct Profile @ 1:12-1:19 // goto field type
        "#]],
    );
}

#[test]
fn resolves_import_alias_cursors_to_imported_definitions() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_import_alias_goto
mod api {
    pub struct User;
}

use api::User as Acc$0ount; // goto("goto import alias")
"#,
        expect![[r#"
            struct User @ 2:16-2:20 // goto import alias
        "#]],
    );
}

#[test]
fn resolves_self_in_impl_signatures_to_impl_self_type() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_impl_self_signature_goto
pub struct User;

impl User {
    pub fn new() -> Se$0lf { // goto("goto impl signature Self")
        User
    }
}
"#,
        expect![[r#"
            struct User @ 1:12-1:16 // goto impl signature Self
        "#]],
    );
}

#[test]
fn resolves_body_local_structs_before_module_structs() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_local_struct_goto
pub struct User;

pub fn make() {
    struct User;
    let _local: Us$0er = Us$1er; // goto("goto local type path"), goto("goto local constructor")
}

pub fn outside() {
    let _outside: User = Us$0er; // goto("goto module constructor")
}
"#,
        expect![[r#"
            struct User @ 4:12-4:16 // goto local type path
            struct User @ 4:12-4:16 // goto local constructor
            struct User @ 1:12-1:16 // goto module constructor
        "#]],
    );
}

#[test]
fn resolves_body_local_enum_variants() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_local_enum_variant_goto
pub fn make() {
    enum Action {
        Start,
        Stop,
    }

    let _action = Action::Sta$0rt; // goto("goto local enum variant")
}
"#,
        expect![[r#"
            variant Start @ 3:9-3:14 // goto local enum variant
        "#]],
    );
}

#[test]
fn resolves_body_local_record_literals() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_local_record_literal_goto
pub struct GlobalId;

pub fn make() {
    struct User {
        id: GlobalId,
    }

    let _user = Us$0er { id: GlobalId }; // goto("goto local record literal")
}
"#,
        expect![[r#"
            struct User @ 4:12-4:16 // goto local record literal
        "#]],
    );
}

#[test]
fn resolves_body_local_value_shadowing_by_scope() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_value_shadowing_goto
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
        let _from_fn = val$0ue(); // goto("goto inner function")
    };

    {
        const helper: Inner = Inner;
        let _from_const = hel$0per; // goto("goto inner const")
    };
}
"#,
        expect![[r#"
            fn value @ 11:12-11:17 // goto inner function
            const helper @ 18:15-18:21 // goto inner const
        "#]],
    );
}

#[test]
fn resolves_parent_body_local_items_from_nested_body_owners() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_nested_body_owner_goto
pub struct GlobalId;

pub fn make() {
    struct Local;
    const DEFAULT: Local = Local;
    static CURRENT: Local = Local;

    fn helper() -> Local {
        DEF$0AULT // goto("goto parent const from nested function")
    }

    const AGAIN: Local = CUR$0RENT; // goto("goto parent static from nested const initializer")
    const FROM_CONST: Local = DEF$0AULT; // goto("goto parent const from nested const initializer")
    static LAST: Local = Lo$0cal; // goto("goto parent type from nested static initializer")
    static FROM_STATIC: Local = DEF$0AULT; // goto("goto parent const from nested static initializer")
}
"#,
        expect![[r#"
            const DEFAULT @ 5:11-5:18 // goto parent const from nested function
            static CURRENT @ 6:12-6:19 // goto parent static from nested const initializer
            const DEFAULT @ 5:11-5:18 // goto parent const from nested const initializer
            struct Local @ 4:12-4:17 // goto parent type from nested static initializer
            const DEFAULT @ 5:11-5:18 // goto parent const from nested static initializer
        "#]],
    );
}

#[test]
fn resolves_body_local_associated_consts_and_types() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_local_assoc_goto
pub struct GlobalId;

pub fn make() {
    struct User;

    impl User {
        const DEFAULT: GlobalId = GlobalId;
        type Id = GlobalId;
    }

    let _default = User::DE$0FAULT; // goto("goto associated const")
    let _typed: User::I$0d = GlobalId; // goto("goto associated type")
}
"#,
        expect![[r#"
            const DEFAULT @ 7:15-7:22 // goto associated const
            type_alias Id @ 8:14-8:16 // goto associated type
        "#]],
    );
}

#[test]
fn resolves_trait_associated_const_qualified_value_paths() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_trait_assoc_const_goto
pub enum StmtKind {
    Let,
}

pub struct Stmt;

pub trait HasAttrs {
    const SUPPORTS_CUSTOM_INNER_ATTRS: bool;
}

impl HasAttrs for StmtKind {
    const SUPPORTS_CUSTOM_INNER_ATTRS: bool = true;
}

impl HasAttrs for Stmt {
    const SUPPORTS_CUSTOM_INNER_ATTRS: bool =
        StmtKind::SUPPORTS$0_CUSTOM_INNER_ATTRS$1; // goto("goto trait associated const"), ty("type at trait associated const")
}

pub fn use_it() {
    let _ = <StmtKind as HasAttrs>::SUPPORTS$0_CUSTOM_INNER_ATTRS$1; // goto("goto qualified trait associated const"), ty("type at qualified trait associated const")
}
"#,
        expect![[r#"
            const SUPPORTS_CUSTOM_INNER_ATTRS @ 12:11-12:38 // goto trait associated const
            bool // type at trait associated const
            const SUPPORTS_CUSTOM_INNER_ATTRS @ 12:11-12:38 // goto qualified trait associated const
            bool // type at qualified trait associated const
        "#]],
    );
}

#[test]
fn resolves_body_local_imported_paths_and_impl_headers() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_local_import_goto
pub struct GlobalId;
pub struct LocalId;

pub fn use_it(id: GlobalId) {
    use crate::GlobalId as RootId;
    use RootId as LocalAlias;

    impl LocalAlias {
        fn local(&self) -> LocalId {
            missing()
        }
    }

    let _typed: Local$0Alias = id; // goto("goto chained body-local import")
    let _local = id.lo$0cal(); // goto("goto imported impl method")
}
"#,
        expect![[r#"
            struct GlobalId @ 1:12-1:20 // goto chained body-local import
            fn local @ 9:12-9:17 // goto imported impl method
        "#]],
    );
}

#[test]
fn resolves_body_let_annotation_paths_with_body_context() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_annotation_goto
pub struct User;

impl User {
    pub fn capture(&self) {
        let _this: Se$0lf = self; // goto("goto body Self annotation")
    }
}

pub fn make() {
    struct User;
    let _: Us$0er = User; // goto("goto wildcard annotation")
    let (_left, _right): (Us$0er, Us$1er) = User; // goto("goto tuple annotation left"), goto("goto tuple annotation right")
}
"#,
        expect![[r#"
            struct User @ 1:12-1:16 // goto body Self annotation
            struct User @ 10:12-10:16 // goto wildcard annotation
            struct User @ 10:12-10:16 // goto tuple annotation left
            struct User @ 10:12-10:16 // goto tuple annotation right
        "#]],
    );
}
