use expect_test::expect;

use super::utils::{check_analysis, check_analysis_with_fake_sysroot};

#[test]
fn hovers_with_evaluated_const_values() {
    check_analysis(
        r#"
        //- /src/lib.rs crate:analysis_hover_const_values
        mod sizes;
        use sizes::CAPACITY;

        type Size = usize;
        /// Number of slots.
        pub const SL$0OTS: Size = (2 + 2) * 4; // hover("const declaration value")

        pub struct Buffer;
        impl Buffer {
            pub const CAPACITY: usize = 1 << (2 + 2u8);
        }

        pub trait Limit {
            const DEFAULT: usize = 0xff & !(1 << 3);
        }
        impl Limit for Buffer {}

        pub fn demo() {
            const LOCAL: usize = { 8 / 2 + 1 };
            let _ = SL$0OTS; // hover("const reference value")
            let _ = Buffer::CAPA$0CITY; // hover("associated const value")
            let _ = Buffer::DE$0FAULT; // hover("trait default const value")
            let _ = LO$0CAL; // hover("body-local const value")
            let _ = CAPA$0CITY; // hover("imported const value")
        }

        //- /src/sizes.rs
        pub const CAPACITY: usize = 0x10 + 0o10 + 0b10;
        "#,
        expect![[r#"
            // const declaration value
            range: 6:11-6:16
            block:
              kind: const
              path: analysis_hover_const_values::SLOTS
              signature:
                pub const SLOTS: Size = 16
              docs:
                Number of slots.

            // const reference value
            range: 20:13-20:18
            block:
              kind: const
              path: analysis_hover_const_values::SLOTS
              signature:
                pub const SLOTS: Size = 16
              docs:
                Number of slots.

            // associated const value
            range: 21:21-21:29
            block:
              kind: const
              path: analysis_hover_const_values::Buffer::CAPACITY
              signature:
                pub const CAPACITY: usize = 16

            // trait default const value
            range: 22:21-22:28
            block:
              kind: const
              path: analysis_hover_const_values::Limit::DEFAULT
              signature:
                const DEFAULT: usize = 247

            // body-local const value
            range: 23:13-23:18
            block:
              kind: const
              signature:
                const LOCAL: usize = 5

            // imported const value
            range: 24:13-24:21
            block:
              kind: const
              path: analysis_hover_const_values::sizes::CAPACITY
              signature:
                pub const CAPACITY: usize = 26
        "#]],
    );
}

#[test]
fn hovers_with_formatted_function_signatures() {
    check_analysis(
        r#"
        //- /src/lib.rs crate:analysis_hover_signatures
        /// Resolves an implementation header.
        pub(crate) fn impl_header_$0with<'query, D, I, R>( // hover("hover long function signature")
            item_paths: &ItemPathQuery<'query, D, I>,
            resolver: &R,
            impl_ref: ImplRef,
        ) -> Result<Option<ImplHeader>, D::Error>
        where
            D: DefMapSource,
            I: ItemStoreSource<'query, Error = D::Error>,
            R: TypePathResolver<Error = D::Error>,
        {
            loop {}
        }
        "#,
        expect![[r#"
            // hover long function signature
            range: 2:15-2:31
            block:
              kind: fn
              path: analysis_hover_signatures::impl_header_with
              signature:
                pub(crate) fn impl_header_with<'query, D, I, R>(
                    item_paths: &ItemPathQuery<'query, D, I>,
                    resolver: &R,
                    impl_ref: ImplRef,
                ) -> Result<Option<ImplHeader>, D::Error>
                where
                    D: DefMapSource,
                    I: ItemStoreSource<'query, Error = D::Error>,
                    R: TypePathResolver<Error = D::Error>,
              docs:
                Resolves an implementation header.
        "#]],
    );
}

#[test]
fn hovers_over_documented_items_and_usages() {
    check_analysis(
        r#"
        //- /src/lib.rs crate:analysis_hover
        /// User account stored by the service.
        pub struct User {
            /// Display name shown in the UI.
            pub name: Profile,
        }

        #[doc = "Public profile data."]
        pub struct Profile;

        /// Builds a user.
        pub fn make_user() -> User {
            User { name: Profile }
        }

        pub fn demo() {
            let user = make_u$0ser(); // hover("hover function")
            let _name = user.na$0me; // hover("hover field")
            let _typed: Us$0er = user; // hover("hover type")
            let lo$0cal = Profile; // hover("hover local")
        }
        "#,
        expect![[r#"
            // hover function
            range: 16:16-16:25
            block:
              kind: fn
              path: analysis_hover::make_user
              signature:
                pub fn make_user() -> User
              docs:
                Builds a user.

            // hover field
            range: 17:22-17:26
            block:
              kind: field
              path: analysis_hover::User
              signature:
                pub name: Profile
              docs:
                Display name shown in the UI.

            // hover type
            range: 18:17-18:21
            block:
              kind: struct
              path: analysis_hover::User
              signature:
                pub struct User {
                    pub name: Profile,
                }
              docs:
                User account stored by the service.

            // hover local
            range: 19:9-19:14
            block:
              kind: variable
              signature:
                let local: Profile
        "#]],
    );
}

#[test]
fn hovers_over_impl_fn_trait_parenthesized_args() {
    check_analysis_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:analysis_hover_fn_trait_args
pub struct AttrVec;

pub fn configure(f$0: impl FnOnce(&mut AttrVec)) { // hover("hover impl FnOnce parenthesized args").in_lib("analysis_hover_fn_trait_args")
    let _ = f;
}
"#,
        expect![[r#"
            // hover impl FnOnce parenthesized args
            range: 3:18-3:19
            block:
              kind: variable
              signature:
                let f: impl FnOnce(&mut AttrVec)
        "#]],
    );
}

#[test]
fn hovers_over_body_macro_calls_without_exposing_generated_internals() {
    check_analysis(
        r#"
        //- /src/lib.rs crate:analysis_hover_body_macros
        pub struct Text;

        /// Internal helper used by macro output.
        pub fn helper(value: u64) -> Text {
            Text
        }

        /// Builds text from a value.
        macro_rules! make_text {
            ($value:expr) => { helper($value) };
        }

        pub fn demo(input: u64) {
            let text = make_$0text!(input); // hover("hover generated macro call")
            text;
        }
        "#,
        expect![[r#"
            // hover generated macro call
            range: 14:16-14:25
            block:
              kind: macro
              path: analysis_hover_body_macros::make_text
              signature:
                macro_definition make_text
              docs:
                Builds text from a value.
        "#]],
    );
}

#[test]
fn hovers_over_enum_variants_and_body_local_items() {
    check_analysis(
        r#"
        //- /src/lib.rs crate:analysis_hover_locals
        pub enum Event {
            /// Event has started.
            Sta$0rted, // hover("hover enum variant declaration")
        }

        pub fn demo() {
            /// Request scoped to this function.
            struct Request {
                /// Request identifier.
                id: Event,
            }

            enum Action {
                /// Local action has started.
                Start(Event),
                Stop,
            }

            impl Request {
                /// Returns the request id.
                fn id(&self) -> Event {
                    Event::Started
                }
            }

            let request: Request;
            let _id = request.i$0d(); // hover("hover body-local method")
            let _field = request.i$0d; // hover("hover body-local field")
            let _event = Event::Sta$0rted; // hover("hover enum variant")
            let _action = Action::Sta$0rt(Event::Started); // hover("hover body-local enum variant")
            let _typed: Re$0quest = request; // hover("hover body-local type")
        }
        "#,
        expect![[r#"
            // hover enum variant declaration
            range: 3:5-3:12
            block:
              kind: variant
              path: analysis_hover_locals::Event::Started
              signature:
                Started
              docs:
                Event has started.

            // hover body-local method
            range: 27:23-27:25
            block:
              kind: method
              signature:
                fn id(&self) -> Event
              docs:
                Returns the request id.

            // hover body-local field
            range: 28:26-28:28
            block:
              kind: field
              signature:
                id: Event
              docs:
                Request identifier.

            // hover enum variant
            range: 29:25-29:32
            block:
              kind: variant
              path: analysis_hover_locals::Event::Started
              signature:
                Started
              docs:
                Event has started.

            // hover body-local enum variant
            range: 30:27-30:32
            block:
              kind: variant
              signature:
                Start(Event)
              docs:
                Local action has started.

            // hover body-local type
            range: 31:17-31:24
            block:
              kind: struct
              signature:
                struct Request {
                    id: Event,
                }
              docs:
                Request scoped to this function.
        "#]],
    );
}

#[test]
fn hovers_over_more_body_local_items() {
    check_analysis(
        r#"
        //- /src/lib.rs crate:analysis_hover_more_locals
        pub struct GlobalId;

        pub fn demo() {
            /// User id alias.
            type Alias = GlobalId;
            /// Local default id.
            const DEFAULT: Alias = GlobalId;
            /// Current id.
            static CURRENT: GlobalId = GlobalId;
            /// Builds local id.
            fn helper() -> Alias {
                DEFAULT
            }

            let _typed: Al$0ias = helper(); // hover("hover body-local alias")
            let _default = DE$0FAULT; // hover("hover body-local const")
            let _current = CU$0RRENT; // hover("hover body-local static")
            let _helper = hel$0per(); // hover("hover body-local free function")
        }
        "#,
        expect![[r#"
            // hover body-local alias
            range: 15:17-15:22
            block:
              kind: type_alias
              signature:
                type Alias = GlobalId
              docs:
                User id alias.

            // hover body-local const
            range: 16:20-16:27
            block:
              kind: const
              signature:
                const DEFAULT: Alias
              docs:
                Local default id.

            // hover body-local static
            range: 17:20-17:27
            block:
              kind: static
              signature:
                static CURRENT: GlobalId
              docs:
                Current id.

            // hover body-local free function
            range: 18:19-18:25
            block:
              kind: fn
              signature:
                fn helper() -> Alias
              docs:
                Builds local id.
        "#]],
    );
}

#[test]
fn hovers_over_documented_module_declarations() {
    check_analysis(
        r#"
        //- /src/lib.rs crate:analysis_hover_modules
        pub mod a$0pi; // hover("hover out-of-line module")

        /// Inline helpers.
        pub mod he$0lpers { // hover("hover inline module")
            //! Inline helper internals.
            pub struct Helper;
        }

        //- /src/api.rs
        //! Public API surface.
        pub struct Api;
        "#,
        expect![[r#"
            // hover out-of-line module
            range: 1:9-1:12
            block:
              kind: module
              path: analysis_hover_modules::api
              signature:
                mod api
              docs:
                Public API surface.

            // hover inline module
            range: 4:9-4:16
            block:
              kind: module
              path: analysis_hover_modules::helpers
              signature:
                mod helpers
              docs:
                Inline helpers.
                Inline helper internals.
        "#]],
    );
}

#[test]
fn hovers_over_crate_root_path_names_and_docs() {
    check_analysis(
        r#"
//- /crates/dep/src/lib.rs crate:dep root:crates/dep
//! Dependency crate docs.
pub struct Thing;

//- /crates/app/src/lib.rs crate:app root:crates/app deps:dep
//! Application crate docs.
pub struct Api;

pub fn use_roots(_: cra$0te::Api, _: de$1p::Thing) {} // hover("hover crate root path").in_lib("app"), hover("hover dependency root path").in_lib("app")
"#,
        expect![[r#"
            // hover crate root path
            range: 4:21-4:26
            block:
              kind: module
              path: app
              signature:
                mod crate
              docs:
                Application crate docs.

            // hover dependency root path
            range: 4:36-4:39
            block:
              kind: module
              path: dep
              signature:
                mod dep
              docs:
                Dependency crate docs.
        "#]],
    );
}

#[test]
fn hovers_over_method_when_unrelated_workspace_crate_has_same_named_trait_method() {
    check_analysis(
        r#"
//- /crates/shared/src/lib.rs crate:shared root:crates/shared
pub struct Maybe<T>(T);
pub struct User;
pub struct Label;

impl<T> Maybe<T> {
    pub fn and_then(self) -> Label {
        Label
    }
}

//- /crates/unrelated/src/lib.rs crate:unrelated root:crates/unrelated deps:shared
pub trait Layer<S> {
    fn and_then(self) -> S;
}

impl<T, S> Layer<S> for shared::Maybe<T> {
    fn and_then(self) -> S {
        loop {}
    }
}

//- /crates/app/src/lib.rs crate:app root:crates/app deps:shared
pub fn demo(maybe: shared::Maybe<shared::User>) {
    let _label = maybe.an$0d_then(); // hover("hover visible method").in_lib("app")
}
"#,
        expect![[r#"
            // hover visible method
            range: 2:24-2:32
            block:
              kind: method
              path: shared::Maybe::and_then
              signature:
                pub fn and_then(self) -> Label
        "#]],
    );
}

#[test]
fn hovers_over_primitive_method_reached_through_deref() {
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

pub struct OwnedText;

impl Project for OwnedText {
    type Target = str;

    fn project(&self) -> &Self::Target {
        missing()
    }
}

//- /app/src/lib.rs crate:app root:app deps:runtime
use runtime::OwnedText;
pub fn use_it(text: OwnedText) {
    let _found = text.con$0tains("needle"); // hover("hover primitive method through Deref").in_lib("app")
}
"#,
        expect![[r#"
            // hover primitive method through Deref
            range: 3:23-3:31
            block:
              kind: method
              path: runtime::contains
              signature:
                pub fn contains(&self, needle: &str) -> bool
              docs:
                Finds a substring.
        "#]],
    );
}

#[test]
fn hovers_over_unkeyed_inherent_associated_paths() {
    check_analysis(
        r#"
//- /runtime/src/lib.rs crate:runtime root:runtime
impl u32 {
    /// Largest represented value.
    pub const MAX: u32 = 0;

    /// Builds an integer from big-endian bytes.
    pub fn from_be_bytes(_bytes: [u8; 4]) -> Self { 0 }
}

//- /app/src/lib.rs crate:app root:app deps:runtime
pub fn use_it(bytes: [u8; 4]) {
    let _max = u32::M$0AX; // hover("hover primitive associated const").in_lib("app")
    let _value = u32::from_be_$0bytes(bytes); // hover("hover primitive associated function").in_lib("app")
}
"#,
        expect![[r#"
            // hover primitive associated const
            range: 2:21-2:24
            block:
              kind: const
              path: runtime::MAX
              signature:
                pub const MAX: u32
              docs:
                Largest represented value.

            // hover primitive associated function
            range: 3:23-3:36
            block:
              kind: method
              path: runtime::from_be_bytes
              signature:
                pub fn from_be_bytes(_bytes: [u8; 4]) -> Self
              docs:
                Builds an integer from big-endian bytes.
        "#]],
    );
}

#[test]
fn hovers_with_bounded_item_previews() {
    check_analysis(
        r#"
        //- /src/lib.rs crate:analysis_hover_previews
        pub struct Pa$0cket { // hover("hover struct preview")
            pub first: u8,
            pub second: u8,
            pub third: u8,
            pub fourth: u8,
            pub fifth: u8,
            pub sixth: u8,
        }

        pub enum Ev$0ent { // hover("hover enum preview")
            Created { id: u64, packet: Packet },
            Deleted(Packet),
            Ping,
            Pong,
            Ack,
            Nack,
        }
        "#,
        expect![[r#"
            // hover struct preview
            range: 1:12-1:18
            block:
              kind: struct
              path: analysis_hover_previews::Packet
              signature:
                pub struct Packet {
                    pub first: u8,
                    pub second: u8,
                    pub third: u8,
                    pub fourth: u8,
                    pub fifth: u8,
                    ...,
                }

            // hover enum preview
            range: 10:10-10:15
            block:
              kind: enum
              path: analysis_hover_previews::Event
              signature:
                pub enum Event {
                    Created { id: u64, packet: Packet },
                    Deleted(Packet),
                    Ping,
                    Pong,
                    Ack,
                    ...,
                }
        "#]],
    );
}
