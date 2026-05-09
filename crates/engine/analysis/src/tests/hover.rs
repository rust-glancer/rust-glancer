use expect_test::expect;

use super::utils::{AnalysisQuery, check_analysis_queries};

#[test]
fn hovers_over_documented_items_and_usages() {
    check_analysis_queries(
        r#"
        //- /Cargo.toml
        [package]
        name = "analysis_hover"
        version = "0.0.0"
        edition = "2024"

        //- /src/lib.rs
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
            let user = make_u$fn_hover$ser();
            let _name = user.na$field_hover$me;
            let _typed: Us$type_hover$er = user;
            let lo$local_hover$cal = Profile;
        }
        "#,
        &[
            AnalysisQuery::hover("hover function", "fn_hover"),
            AnalysisQuery::hover("hover field", "field_hover"),
            AnalysisQuery::hover("hover type", "type_hover"),
            AnalysisQuery::hover("hover local", "local_hover"),
        ],
        expect![[r#"
            hover function
            - range: 16:16-16:25
            - block:
              kind: fn
              path: analysis_hover::make_user
              signature:
                pub fn make_user() -> User
              docs:
                Builds a user.

            hover field
            - range: 17:17-17:26
            - block:
              kind: field
              path: analysis_hover::User
              signature:
                pub name: Profile
              docs:
                Display name shown in the UI.

            hover type
            - range: 18:17-18:21
            - block:
              kind: struct
              path: analysis_hover::User
              signature:
                pub struct User {
                    pub name: Profile,
                }
              docs:
                User account stored by the service.

            hover local
            - range: 19:9-19:14
            - block:
              kind: variable
              signature:
                let local: Profile
        "#]],
    );
}

#[test]
fn hovers_over_enum_variants_and_body_local_items() {
    check_analysis_queries(
        r#"
        //- /Cargo.toml
        [package]
        name = "analysis_hover_locals"
        version = "0.0.0"
        edition = "2024"

        //- /src/lib.rs
        pub enum Event {
            /// Event has started.
            Sta$variant_decl_hover$rted,
        }

        pub fn demo() {
            /// Request scoped to this function.
            struct Request {
                /// Request identifier.
                id: Event,
            }

            impl Request {
                /// Returns the request id.
                fn id(&self) -> Event {
                    Event::Started
                }
            }

            let request: Request;
            let _id = request.i$method_hover$d();
            let _field = request.i$local_field_hover$d;
            let _event = Event::Sta$variant_hover$rted;
            let _typed: Re$local_type_hover$quest = request;
        }
        "#,
        &[
            AnalysisQuery::hover("hover body-local method", "method_hover"),
            AnalysisQuery::hover("hover body-local field", "local_field_hover"),
            AnalysisQuery::hover("hover enum variant declaration", "variant_decl_hover"),
            AnalysisQuery::hover("hover enum variant", "variant_hover"),
            AnalysisQuery::hover("hover body-local type", "local_type_hover"),
        ],
        expect![[r#"
            hover body-local method
            - range: 21:15-21:27
            - block:
              kind: method
              signature:
                fn id(&self) -> Event
              docs:
                Returns the request id.

            hover body-local field
            - range: 22:18-22:28
            - block:
              kind: field
              signature:
                id: Event
              docs:
                Request identifier.

            hover enum variant declaration
            - range: 3:5-3:12
            - block:
              kind: variant
              path: analysis_hover_locals::Event::Started
              signature:
                Started
              docs:
                Event has started.

            hover enum variant
            - range: 23:25-23:32
            - block:
              kind: variant
              path: analysis_hover_locals::Event::Started
              signature:
                Started
              docs:
                Event has started.

            hover body-local type
            - range: 24:17-24:24
            - block:
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
fn hovers_over_documented_module_declarations() {
    check_analysis_queries(
        r#"
        //- /Cargo.toml
        [package]
        name = "analysis_hover_modules"
        version = "0.0.0"
        edition = "2024"

        //- /src/lib.rs
        pub mod a$out_of_line_module_hover$pi;

        /// Inline helpers.
        pub mod he$inline_module_hover$lpers {
            //! Inline helper internals.
            pub struct Helper;
        }

        //- /src/api.rs
        //! Public API surface.
        pub struct Api;
        "#,
        &[
            AnalysisQuery::hover("hover out-of-line module", "out_of_line_module_hover"),
            AnalysisQuery::hover("hover inline module", "inline_module_hover"),
        ],
        expect![[r#"
            hover out-of-line module
            - range: 1:9-1:12
            - block:
              kind: module
              path: analysis_hover_modules::api
              signature:
                mod api
              docs:
                Public API surface.

            hover inline module
            - range: 4:9-4:16
            - block:
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
//! Dependency crate docs.
pub struct Thing;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
//! Application crate docs.
pub struct Api;

pub fn use_roots(_: cra$crate_root_hover$te::Api, _: de$dep_root_hover$p::Thing) {}
"#,
        &[
            AnalysisQuery::hover("hover crate root path", "crate_root_hover").in_lib("app"),
            AnalysisQuery::hover("hover dependency root path", "dep_root_hover").in_lib("app"),
        ],
        expect![[r#"
            hover crate root path
            - range: 4:21-4:26
            - block:
              kind: module
              path: app
              signature:
                mod crate
              docs:
                Application crate docs.

            hover dependency root path
            - range: 4:36-4:39
            - block:
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
fn hovers_with_bounded_item_previews() {
    check_analysis_queries(
        r#"
        //- /Cargo.toml
        [package]
        name = "analysis_hover_previews"
        version = "0.0.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct Pa$struct_preview$cket {
            pub first: u8,
            pub second: u8,
            pub third: u8,
            pub fourth: u8,
            pub fifth: u8,
            pub sixth: u8,
        }

        pub enum Ev$enum_preview$ent {
            Created { id: u64, packet: Packet },
            Deleted(Packet),
            Ping,
            Pong,
            Ack,
            Nack,
        }
        "#,
        &[
            AnalysisQuery::hover("hover struct preview", "struct_preview"),
            AnalysisQuery::hover("hover enum preview", "enum_preview"),
        ],
        expect![[r#"
            hover struct preview
            - range: 1:12-1:18
            - block:
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

            hover enum preview
            - range: 10:10-10:15
            - block:
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
