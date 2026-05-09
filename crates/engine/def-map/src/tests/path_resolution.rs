use expect_test::expect;

use super::utils::{self, PathResolutionQuery};

#[test]
fn resolves_paths_against_frozen_def_map() {
    utils::check_project_path_resolution(
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
pub trait ExternalTrait {}

mod hidden {
    pub trait HiddenTrait {}
}

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
use dep::ExternalTrait as ImportedTrait;

pub struct Root;

pub mod api {
    pub struct LocalType;
    struct PrivateType;
    pub const LOCAL_CONST: u8 = 0;

    pub mod child {
        pub struct Child;
    }
}
"#,
        &[
            PathResolutionQuery::lib("app", "crate", "dep::ExternalTrait"),
            PathResolutionQuery::lib("app", "crate", "::dep::ExternalTrait"),
            PathResolutionQuery::lib("app", "crate", "ImportedTrait"),
            PathResolutionQuery::lib("app", "crate", "crate::api::LOCAL_CONST"),
            PathResolutionQuery::lib("app", "crate::api::child", "self::Child"),
            PathResolutionQuery::lib("app", "crate::api::child", "super::LocalType"),
            PathResolutionQuery::lib("app", "crate::api::child", "super::PrivateType"),
            PathResolutionQuery::lib("app", "crate", "crate::api::PrivateType"),
            PathResolutionQuery::lib("app", "crate", "dep::hidden::HiddenTrait"),
            PathResolutionQuery::lib("app", "crate", "missing::Thing"),
            PathResolutionQuery::lib("app", "crate", "dep::missing::Thing"),
            PathResolutionQuery::lib("app", "crate", "crate::dep::ExternalTrait"),
        ],
        expect![[r#"
            app [lib] crate resolves dep::ExternalTrait -> trait dep[lib]::crate::ExternalTrait
            app [lib] crate resolves ::dep::ExternalTrait -> trait dep[lib]::crate::ExternalTrait
            app [lib] crate resolves ImportedTrait -> trait dep[lib]::crate::ExternalTrait
            app [lib] crate resolves crate::api::LOCAL_CONST -> const app[lib]::crate::api::LOCAL_CONST
            app [lib] crate::api::child resolves self::Child -> struct app[lib]::crate::api::child::Child
            app [lib] crate::api::child resolves super::LocalType -> struct app[lib]::crate::api::LocalType
            app [lib] crate::api::child resolves super::PrivateType -> struct app[lib]::crate::api::PrivateType
            app [lib] crate resolves crate::api::PrivateType -> <none> (unresolved at segment #2)
            app [lib] crate resolves dep::hidden::HiddenTrait -> <none> (unresolved at segment #1)
            app [lib] crate resolves missing::Thing -> <none> (unresolved at segment #0)
            app [lib] crate resolves dep::missing::Thing -> <none> (unresolved at segment #1)
            app [lib] crate resolves crate::dep::ExternalTrait -> <none> (unresolved at segment #1)
        "#]],
    );
}

#[test]
fn resolves_bin_target_roots_and_dependencies() {
    utils::check_project_path_resolution(
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
mod cli;

fn main() {}

//- /crates/app/src/cli.rs
pub struct Thing;
"#,
        &[
            PathResolutionQuery::bin("app", "crate", "app::Api"),
            PathResolutionQuery::bin("app", "crate", "dep::Thing"),
            PathResolutionQuery::bin("app", "crate", "cli::Thing"),
        ],
        expect![[r#"
            app [bin] crate resolves app::Api -> struct app[lib]::crate::Api
            app [bin] crate resolves dep::Thing -> struct dep[lib]::crate::Thing
            app [bin] crate resolves cli::Thing -> struct app[bin]::crate::cli::Thing
        "#]],
    );
}

#[test]
fn resolves_injected_sysroot_extern_roots() {
    utils::check_project_path_resolution_with_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
use Option as Maybe;

pub struct App;

pub mod local_shadow {
    pub struct Vec;
}

//- /sysroot/library/core/src/lib.rs
pub mod marker {
    pub struct Core;
    pub struct CorePrelude;
}

pub mod option {
    pub enum Option {}
}

pub mod prelude {
    pub mod rust_2024 {
        pub use crate::marker::CorePrelude;
    }
}

//- /sysroot/library/alloc/src/lib.rs
pub mod marker {
    pub struct Alloc;
    pub struct Vec;
}

//- /sysroot/library/std/src/lib.rs
pub mod marker {
    pub struct Std;
    pub struct StdPrelude;
}

pub mod prelude {
    pub mod rust_2024 {
        pub use crate::marker::StdPrelude;
        pub use core::marker::CorePrelude;
        pub use core::option::Option;
        pub use alloc::marker::Vec;
    }
}
"#,
        &[
            PathResolutionQuery::lib("app", "crate", "std::marker::Std"),
            PathResolutionQuery::lib("app", "crate", "core::marker::Core"),
            PathResolutionQuery::lib("app", "crate", "alloc::marker::Alloc"),
            PathResolutionQuery::lib("app", "crate", "StdPrelude"),
            PathResolutionQuery::lib("app", "crate", "CorePrelude"),
            PathResolutionQuery::lib("app", "crate", "Vec"),
            PathResolutionQuery::lib("app", "crate", "Maybe"),
            PathResolutionQuery::lib("app", "crate::local_shadow", "Vec"),
            PathResolutionQuery::lib("app", "crate", "::StdPrelude"),
            PathResolutionQuery::lib("std", "crate", "core::marker::Core"),
            PathResolutionQuery::lib("std", "crate", "alloc::marker::Alloc"),
            PathResolutionQuery::lib("alloc", "crate", "core::marker::Core"),
        ],
        expect![[r#"
            app [lib] crate resolves std::marker::Std -> struct std[lib]::crate::marker::Std
            app [lib] crate resolves core::marker::Core -> struct core[lib]::crate::marker::Core
            app [lib] crate resolves alloc::marker::Alloc -> struct alloc[lib]::crate::marker::Alloc
            app [lib] crate resolves StdPrelude -> struct std[lib]::crate::marker::StdPrelude
            app [lib] crate resolves CorePrelude -> struct core[lib]::crate::marker::CorePrelude
            app [lib] crate resolves Vec -> struct alloc[lib]::crate::marker::Vec
            app [lib] crate resolves Maybe -> enum core[lib]::crate::option::Option
            app [lib] crate::local_shadow resolves Vec -> struct app[lib]::crate::local_shadow::Vec
            app [lib] crate resolves ::StdPrelude -> <none> (unresolved at segment #0)
            std [lib] crate resolves core::marker::Core -> struct core[lib]::crate::marker::Core
            std [lib] crate resolves alloc::marker::Alloc -> struct alloc[lib]::crate::marker::Alloc
            alloc [lib] crate resolves core::marker::Core -> struct core[lib]::crate::marker::Core
        "#]],
    );
}

#[test]
fn selects_standard_prelude_from_package_edition() {
    utils::check_project_path_resolution_with_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2021"

//- /src/lib.rs
pub struct App;

//- /sysroot/library/core/src/lib.rs
pub struct Core;

//- /sysroot/library/alloc/src/lib.rs
pub struct Alloc;

//- /sysroot/library/std/src/lib.rs
pub mod marker {
    pub struct LegacyPrelude;
    pub struct NewPrelude;
}

pub mod prelude {
    pub mod rust_2021 {
        pub use crate::marker::LegacyPrelude;
    }

    pub mod rust_2024 {
        pub use crate::marker::NewPrelude;
    }
}
"#,
        &[
            PathResolutionQuery::lib("app", "crate", "LegacyPrelude"),
            PathResolutionQuery::lib("app", "crate", "NewPrelude"),
        ],
        expect![[r#"
            app [lib] crate resolves LegacyPrelude -> struct std[lib]::crate::marker::LegacyPrelude
            app [lib] crate resolves NewPrelude -> <none> (unresolved at segment #0)
        "#]],
    );
}

#[test]
fn falls_back_to_extern_roots_when_wrong_namespace_bindings_match_first_segment() {
    utils::check_project_path_resolution(
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
pub trait ExternalTrait {}

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
pub mod value_shadow {
    pub const dep: u8 = 0;
}

pub mod macro_shadow {
    macro_rules! dep {
        () => {};
    }
}

pub mod type_shadow {
    pub mod dep {}
}
"#,
        &[
            PathResolutionQuery::lib("app", "crate::value_shadow", "dep::ExternalTrait"),
            PathResolutionQuery::lib("app", "crate::macro_shadow", "dep::ExternalTrait"),
            PathResolutionQuery::lib("app", "crate::type_shadow", "dep::ExternalTrait"),
        ],
        expect![[r#"
            app [lib] crate::value_shadow resolves dep::ExternalTrait -> trait dep[lib]::crate::ExternalTrait
            app [lib] crate::macro_shadow resolves dep::ExternalTrait -> trait dep[lib]::crate::ExternalTrait
            app [lib] crate::type_shadow resolves dep::ExternalTrait -> <none> (unresolved at segment #1)
        "#]],
    );
}
