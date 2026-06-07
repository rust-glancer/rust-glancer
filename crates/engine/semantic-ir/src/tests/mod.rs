mod utils;

use expect_test::expect;

use self::utils::{
    SemanticQuery, check_project_semantic_ir, check_project_semantic_queries,
    check_project_semantic_queries_with_sysroot,
};

#[test]
fn dumps_semantic_ir_signatures() {
    check_project_semantic_ir(
        r#"
//- /Cargo.toml
[package]
name = "semantic_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User<T> {
    pub id: UserId,
    payload: Option<T>,
}

pub struct UserId(u64);

pub enum LoadState<E> {
    Empty,
    Loaded(User),
    Failed { error: E },
}

pub trait Repository<T>
where
    T: Clone,
{
    type Error;
    const KIND: &'static str;
    fn get(&self, id: UserId) -> Result<T, Self::Error>;
}

pub struct DbRepository<T>(T);

impl<T> Repository<T> for DbRepository<T>
where
    T: Clone,
{
    type Error = DbError;
    const KIND: &'static str = "db";
    fn get(&self, id: UserId) -> Result<T, DbError> {
        todo!()
    }
}

pub struct DbError;

pub type UserResult<T> = Result<User<T>, DbError>;
pub const DEFAULT_ID: UserId = UserId(0);
pub static mut CACHE_READY: bool = false;
"#,
        expect![[r#"
            package semantic_fixture

            semantic_fixture [lib]
            crate
            - pub struct User<T>
              - pub field id: UserId
              - field payload: Option<T>
            - pub struct UserId
              - field #0: u64
            - pub enum LoadState<E>
              - variant Empty
              - variant Loaded
                - field #0: User
              - variant Failed
                - field error: E
            - pub trait Repository<T> where T: Clone
              - type Error
              - const KIND: &'static str
              - fn get(&self, id: UserId) -> Result<T, Self::Error>
            - pub struct DbRepository<T>
              - field #0: T
            - pub struct DbError
            - pub type UserResult<T> = Result<User<T>, DbError>
            - pub const DEFAULT_ID: UserId
            - pub static mut CACHE_READY: bool
            - impl<T> Repository<T> for DbRepository<T> where T: Clone
              - type Error = DbError
              - const KIND: &'static str
              - fn get(&self, id: UserId) -> Result<T, DbError>
        "#]],
    );
}

#[test]
fn preserves_absolute_type_path_prefixes() {
    check_project_semantic_ir(
        r#"
//- /Cargo.toml
[package]
name = "absolute_type_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Root;
pub struct UsesAbsolute(::absolute_type_fixture::Root);
pub type AbsoluteAlias = ::absolute_type_fixture::Root;
"#,
        expect![[r#"
            package absolute_type_fixture

            absolute_type_fixture [lib]
            crate
            - pub struct Root
            - pub struct UsesAbsolute
              - field #0: ::absolute_type_fixture::Root
            - pub type AbsoluteAlias = ::absolute_type_fixture::Root
        "#]],
    );
}

#[test]
fn lowers_macro_generated_signatures_and_impls() {
    check_project_semantic_ir(
        r#"
//- /Cargo.toml
[package]
name = "generated_semantic_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_generated {
    () => {
        pub struct Generated<T> {
            pub value: T,
        }

        impl<T> Generated<T> {
            pub fn new(value: T) -> Self {
                Self { value }
            }
        }
    };
}

make_generated!();
"#,
        expect![[r#"
            package generated_semantic_fixture

            generated_semantic_fixture [lib]
            crate
            - pub struct Generated<T>
              - pub field value: T
            - impl<T> Generated<T>
              - pub fn new(value: T) -> Self
        "#]],
    );
}

#[test]
fn resolves_cross_crate_impl_queries() {
    check_project_semantic_queries(
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
pub trait ExternalTrait {
    fn required(&self);
    fn defaulted(&self) {}
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

pub struct Local;

impl Local {
    pub fn new() -> Self {
        Local
    }
}

impl ImportedTrait for Local {
    fn required(&self) {}
}
"#,
        &[SemanticQuery::lib("app", "Local")],
        expect![[r#"
            query app [lib] crate resolves Local -> struct app[lib]::crate::Local
            impls
            - impl ImportedTrait for Local
            - impl Local
            trait impls
            - impl ImportedTrait for Local => trait dep[lib]::crate::ExternalTrait
            traits
            - trait dep[lib]::crate::ExternalTrait
            inherent functions
            - fn impl Local::new
            trait functions
            - fn trait dep[lib]::crate::ExternalTrait::defaulted
            - fn trait dep[lib]::crate::ExternalTrait::required
            trait impl functions
            - fn impl ImportedTrait for Local::required
        "#]],
    );
}

#[test]
fn resolves_core_prelude_trait_impl_headers() {
    check_project_semantic_queries_with_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct App;

//- /sysroot/library/core/src/lib.rs
extern crate self as core;

pub mod marker {
    pub trait Marker {
        fn mark(&self);
    }
}

pub mod prelude {
    pub mod rust_2024 {
        pub use crate::marker::Marker;
    }
}

pub struct CoreType;

impl Marker for CoreType {
    fn mark(&self) {}
}

//- /sysroot/library/alloc/src/lib.rs
pub struct Alloc;

//- /sysroot/library/std/src/lib.rs
pub mod prelude {
    pub mod rust_2024 {}
}
"#,
        &[SemanticQuery::lib("core", "CoreType")],
        expect![[r#"
            query core [lib] crate resolves CoreType -> struct core[lib]::crate::CoreType
            impls
            - impl Marker for CoreType
            trait impls
            - impl Marker for CoreType => trait core[lib]::crate::marker::Marker
            traits
            - trait core[lib]::crate::marker::Marker
            inherent functions
            - <none>
            trait functions
            - fn trait core[lib]::crate::marker::Marker::mark
            trait impl functions
            - fn impl Marker for CoreType::mark
        "#]],
    );
}

#[test]
fn resolves_alloc_impl_headers_through_core_prelude() {
    check_project_semantic_queries_with_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct App;

//- /sysroot/library/core/src/lib.rs
extern crate self as core;

pub mod marker {
    pub trait Marker {
        fn mark(&self);
    }
}

pub mod prelude {
    pub mod rust_2024 {
        pub use crate::marker::Marker;
    }
}

//- /sysroot/library/alloc/src/lib.rs
pub struct AllocType;

impl Marker for AllocType {
    fn mark(&self) {}
}

//- /sysroot/library/std/src/lib.rs
pub mod prelude {
    pub mod rust_2024 {}
}
"#,
        &[SemanticQuery::lib("alloc", "AllocType")],
        expect![[r#"
            query alloc [lib] crate resolves AllocType -> struct alloc[lib]::crate::AllocType
            impls
            - impl Marker for AllocType
            trait impls
            - impl Marker for AllocType => trait core[lib]::crate::marker::Marker
            traits
            - trait core[lib]::crate::marker::Marker
            inherent functions
            - <none>
            trait functions
            - fn trait core[lib]::crate::marker::Marker::mark
            trait impl functions
            - fn impl Marker for AllocType::mark
        "#]],
    );
}

#[test]
fn target_queries_exclude_impls_from_unrelated_workspace_targets() {
    check_project_semantic_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/shared", "crates/app", "crates/other"]
resolver = "3"

//- /crates/shared/Cargo.toml
[package]
name = "shared"
version = "0.1.0"
edition = "2024"

//- /crates/shared/src/lib.rs
pub struct Maybe;

impl Maybe {
    pub fn is_some(&self) -> bool {
        true
    }
}

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
shared = { path = "../shared" }

//- /crates/app/src/lib.rs
use shared::Maybe;

pub trait AppExt {
    fn and_then(&self);
}

impl AppExt for Maybe {
    fn and_then(&self) {}
}

//- /crates/other/Cargo.toml
[package]
name = "other"
version = "0.1.0"
edition = "2024"

[dependencies]
shared = { path = "../shared" }

//- /crates/other/src/lib.rs
use shared::Maybe;

pub trait OtherExt {
    fn and_then(&self);
}

impl OtherExt for Maybe {
    fn and_then(&self) {}
}
"#,
        &[SemanticQuery::lib("app", "shared::Maybe")],
        expect![[r#"
            query app [lib] crate resolves shared::Maybe -> struct shared[lib]::crate::Maybe
            impls
            - impl AppExt for Maybe
            - impl Maybe
            trait impls
            - impl AppExt for Maybe => trait app[lib]::crate::AppExt
            traits
            - trait app[lib]::crate::AppExt
            inherent functions
            - fn impl Maybe::is_some
            trait functions
            - fn trait app[lib]::crate::AppExt::and_then
            trait impl functions
            - fn impl AppExt for Maybe::and_then
        "#]],
    );
}

#[test]
fn resolves_bin_queries_to_sibling_lib_and_dependencies() {
    check_project_semantic_queries(
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

impl Thing {
    pub fn external(&self) {}
}

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

impl Api {
    pub fn local(&self) {}
}

//- /crates/app/src/main.rs
fn main() {}
"#,
        &[
            SemanticQuery::bin("app", "app::Api"),
            SemanticQuery::bin("app", "dep::Thing"),
        ],
        expect![[r#"
            query app [bin] crate resolves app::Api -> struct app[lib]::crate::Api
            impls
            - impl Api
            trait impls
            - <none>
            traits
            - <none>
            inherent functions
            - fn impl Api::local
            trait functions
            - <none>
            trait impl functions
            - <none>


            query app [bin] crate resolves dep::Thing -> struct dep[lib]::crate::Thing
            impls
            - impl Thing
            trait impls
            - <none>
            traits
            - <none>
            inherent functions
            - fn impl Thing::external
            trait functions
            - <none>
            trait impl functions
            - <none>
        "#]],
    );
}

#[test]
fn resolves_module_scoped_semantic_queries() {
    check_project_semantic_queries(
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
pub trait ExternalTrait {
    fn required(&self);
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

pub mod api {
    pub struct Local;

    impl Local {
        pub fn local_method(&self) {}
    }

    impl crate::ImportedTrait for Local {
        fn required(&self) {}
    }
}

mod consumer {
    use crate::api::Local as ImportedLocal;
}
"#,
        &[SemanticQuery::lib_from(
            "app",
            "crate::consumer",
            "ImportedLocal",
        )],
        expect![[r#"
            query app [lib] crate::consumer resolves ImportedLocal -> struct app[lib]::crate::api::Local
            impls
            - impl Local
            - impl crate::ImportedTrait for Local
            trait impls
            - impl crate::ImportedTrait for Local => trait dep[lib]::crate::ExternalTrait
            traits
            - trait dep[lib]::crate::ExternalTrait
            inherent functions
            - fn impl Local::local_method
            trait functions
            - fn trait dep[lib]::crate::ExternalTrait::required
            trait impl functions
            - fn impl crate::ImportedTrait for Local::required
        "#]],
    );
}
