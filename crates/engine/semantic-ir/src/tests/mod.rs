mod trait_impl_lookup;
mod utils;

use std::fmt::Write as _;

use expect_test::expect;
use rg_std::UniqueVec;

use self::utils::{
    SemanticQuery, check_project_semantic_ir, check_project_semantic_queries,
    check_project_semantic_queries_with_sysroot,
};

#[test]
fn high_target_fanout_shares_results_by_ordered_dependency_set() {
    let mut retained_entries = Vec::new();

    for target_count in [1_usize, 4, 12] {
        let mut app_manifest = r#"
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[dev-dependencies]
dep = { path = "../dep" }
"#
        .to_owned();
        let mut test_sources = String::new();
        for target_idx in 0..target_count {
            writeln!(
                app_manifest,
                r#"
[[test]]
name = "test-{target_idx}"
path = "tests/test_{target_idx}.rs"
"#,
            )
            .expect("fixture manifest writes should succeed");
            writeln!(
                test_sources,
                r#"
//- /app/tests/test_{target_idx}.rs
use dep::Shared;

struct Local{target_idx};

impl Shared for Local{target_idx} {{
    fn shared(&self) {{}}
}}
"#,
            )
            .expect("fixture source writes should succeed");
        }

        let fixture_source = format!(
            r#"
//- /Cargo.toml
[workspace]
members = ["app", "dep"]
resolver = "3"

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub trait Shared {{
    fn shared(&self);
}}

pub struct SharedType;

impl Shared for SharedType {{
    fn shared(&self) {{}}
}}

//- /app/Cargo.toml
{app_manifest}
//- /app/src/lib.rs
pub struct Library;
{test_sources}
"#,
        );
        let fixture = crate::testonly::SemanticIrFixture::build(&fixture_source);
        let dep = fixture
            .def_map_fixture()
            .crate_ref("dep", rg_workspace::TargetKind::Lib);
        let shared_trait = fixture
            .resident_crate_ir(dep)
            .expect("dependency semantic store should exist")
            .traits_with_refs()
            .next()
            .expect("dependency should declare one trait")
            .0;
        let shared_type = fixture
            .resident_crate_ir(dep)
            .expect("dependency semantic store should exist")
            .semantic_items()
            .find_map(|item| {
                (item.name()?.as_str() == "SharedType")
                    .then(|| item.type_def())
                    .flatten()
            })
            .expect("dependency should declare SharedType");
        let (app_package_idx, app_package) = fixture
            .parse_db()
            .packages()
            .iter()
            .enumerate()
            .find(|(_, package)| package.package_name() == "app")
            .expect("app parse package should exist");
        let test_crates = app_package
            .targets()
            .iter()
            .filter(|target| target.kind == rg_workspace::TargetKind::Test)
            .map(|target| rg_ir_model::CrateRef {
                package: rg_def_map::PackageSlot(app_package_idx),
                crate_id: rg_ir_model::CrateId(target.id.0),
            })
            .collect::<Vec<_>>();

        let def_maps = fixture
            .def_map_db()
            .read_txn(rg_def_map::DefMapLoader::resident_only(
                "resident fanout fixture",
            ));
        let items = fixture
            .semantic_ir_db()
            .read_txn(crate::SemanticIrLoader::resident_only(
                "resident fanout fixture",
            ));
        let cache = crate::ItemLookupQueryCache::new();
        let dependency_sets = test_crates
            .iter()
            .map(|&use_site| {
                crate::CrateItemQuery::new(&def_maps, &items, use_site)
                    .visible_stores()
                    .expect("test target visible stores should load")
                    .into_iter()
                    .map(crate::ItemStore::crate_ref)
                    .filter(|&crate_ref| crate_ref != use_site)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let local_types = test_crates
            .iter()
            .map(|&use_site| {
                items
                    .items(use_site)
                    .expect("test target semantic package should load")
                    .expect("test target semantic store should exist")
                    .semantic_items()
                    .find_map(|item| {
                        let name = item.name()?;
                        if !name.as_str().starts_with("Local") {
                            return None;
                        }
                        item.type_def()
                    })
                    .expect("test target should declare its local receiver type")
            })
            .collect::<Vec<_>>();
        let queries = test_crates
            .iter()
            .map(|&use_site| {
                crate::ItemLookupQuery::build_with_cache(
                    &crate::CrateItemQuery::new(&def_maps, &items, use_site),
                    &cache,
                )
                .expect("test target lookup query should build")
            })
            .collect::<Vec<_>>();
        for query in &queries {
            let candidates = query
                .trait_impl_candidates_for_self_head(
                    shared_trait,
                    Some(crate::TraitImplSelfHead::Adt(shared_type)),
                )
                .expect("shared dependency trait should be visible from every test target");
            let candidate = candidates
                .as_one()
                .expect("SharedType should have one direct dependency impl");
            assert_eq!(candidate.impl_ref.origin.as_crate_ref(), Some(dep));
        }

        let cache_stats = cache.stats();
        let distinct_dependency_sets = dependency_sets.iter().cloned().collect::<UniqueVec<_>>();
        let distinct_dependency_count = distinct_dependency_sets.len();
        assert_eq!(
            cache_stats.dependency_cache_constructions, distinct_dependency_count,
            "target_count={target_count}, cache_stats={cache_stats:?}, dependency_sets={dependency_sets:?}",
        );
        assert_eq!(
            cache_stats.dependency_cache_reuses,
            target_count - distinct_dependency_count,
            "each ordered dependency set should construct only one shared result cache",
        );
        assert_eq!(
            cache_stats.dependency_result_misses,
            distinct_dependency_count
        );
        assert_eq!(
            cache_stats.dependency_result_hits,
            target_count - distinct_dependency_count,
        );
        assert!(
            distinct_dependency_count <= 2,
            "dependency cache count should stay bounded as sibling test targets grow",
        );

        // Each query must still add only its own local overlay after sharing dependency results.
        // Looking up one target's receiver from a sibling query must not reuse that local impl.
        for ((query, use_site), local_type) in queries.iter().zip(&test_crates).zip(&local_types) {
            let local_impls = query.trait_impls_for_type(*local_type);
            let local_impl = local_impls
                .as_one()
                .expect("each test target should find exactly its own local trait impl");
            assert_eq!(local_impl.trait_ref, shared_trait);
            assert_eq!(local_impl.impl_ref.origin.as_crate_ref(), Some(*use_site));
        }
        if queries.len() > 1 {
            assert!(
                queries[1].trait_impls_for_type(local_types[0]).is_empty(),
                "a shared dependency cache must not expose another target's local impl",
            );
        }

        let semantic_stats = fixture.semantic_ir_db().stats();
        assert_eq!(semantic_stats.lookup_index_count, target_count + 2);
        retained_entries.push(semantic_stats.lookup_index_entry_count);
    }

    assert_eq!(
        (retained_entries[1] - retained_entries[0]) * 8,
        (retained_entries[2] - retained_entries[1]) * 3,
        "retained local-index entries should grow linearly with target-local declarations",
    );
}

#[test]
fn proc_macro_exports_link_to_semantic_implementation_functions() {
    let fixture = crate::testonly::SemanticIrFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "semantic_proc_macros"
version = "0.1.0"
edition = "2024"

[lib]
proc-macro = true

//- /src/lib.rs
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
"#,
    );
    let crate_ref = fixture
        .def_map_fixture()
        .crate_ref("semantic_proc_macros", rg_workspace::TargetKind::ProcMacro);
    let def_map = fixture
        .resident_def_map(crate_ref)
        .expect("proc-macro def map should exist");
    let items = fixture
        .resident_crate_ir(crate_ref)
        .expect("proc-macro semantic items should exist");

    let mut function_names = items
        .functions()
        .iter()
        .map(|function| function.name.to_string())
        .collect::<Vec<_>>();
    function_names.sort();
    assert_eq!(function_names, ["emit", "stored", "traced"]);

    for (export_name, implementation_name) in
        [("emit", "emit"), ("traced", "traced"), ("Stored", "stored")]
    {
        let export = def_map
            .local_def_refs()
            .find(|local_def_ref| {
                def_map
                    .local_def(local_def_ref.local_def)
                    .is_some_and(|data| {
                        data.kind == rg_def_map::LocalDefKind::MacroDefinition
                            && data.name.as_str() == export_name
                    })
            })
            .expect("proc-macro export should exist");
        assert!(
            items.item_for_local_def(export.local_def).is_none(),
            "macro export `{export_name}` must not own a semantic function",
        );

        let implementation = def_map
            .macro_definition(export.local_def)
            .and_then(|data| data.proc_macro_implementation())
            .expect("proc-macro export should retain its implementation identity");
        let implementation_def = def_map
            .local_def(implementation)
            .expect("proc-macro implementation definition should exist");
        assert_eq!(implementation_def.name.as_str(), implementation_name);

        let Some(rg_ir_model::ItemId::Function(function)) =
            items.item_for_local_def(implementation)
        else {
            panic!("proc-macro implementation `{implementation_name}` should lower as a function");
        };
        assert_eq!(
            items
                .function_data(function)
                .expect("proc-macro implementation function should exist")
                .name
                .as_str(),
            implementation_name,
        );
    }
}

#[test]
fn proc_macro_implementation_stores_do_not_cross_the_dependency_boundary() {
    let fixture = crate::testonly::SemanticIrFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["app", "runtime", "derive_macro", "parser"]
resolver = "3"

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
runtime = { path = "../runtime" }

//- /app/src/lib.rs
pub struct App;

//- /runtime/Cargo.toml
[package]
name = "runtime"
version = "0.1.0"
edition = "2024"

[dependencies]
derive_macro = { path = "../derive_macro" }

//- /runtime/src/lib.rs
pub struct Runtime;

//- /derive_macro/Cargo.toml
[package]
name = "derive_macro"
version = "0.1.0"
edition = "2024"

[lib]
proc-macro = true

[dependencies]
parser = { path = "../parser" }

//- /derive_macro/src/lib.rs
extern crate proc_macro;

struct Implementation;

impl Implementation {
    fn host_only() {}
}

#[proc_macro]
pub fn emit(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    input
}

//- /parser/Cargo.toml
[package]
name = "parser"
version = "0.1.0"
edition = "2024"

//- /parser/src/lib.rs
pub struct Parser;
"#,
    );
    let crate_ref =
        |package, target_kind| fixture.def_map_fixture().crate_ref(package, target_kind);
    let app = crate_ref("app", rg_workspace::TargetKind::Lib);
    let runtime = crate_ref("runtime", rg_workspace::TargetKind::Lib);
    let derive_macro = crate_ref("derive_macro", rg_workspace::TargetKind::ProcMacro);
    let parser = crate_ref("parser", rg_workspace::TargetKind::Lib);
    let def_maps = fixture
        .def_map_db()
        .read_txn(rg_def_map::DefMapLoader::resident_only(
            "resident item visibility fixture",
        ));
    let items = fixture
        .semantic_ir_db()
        .read_txn(crate::SemanticIrLoader::resident_only(
            "resident item visibility fixture",
        ));
    let visible_from = |use_site| {
        let mut crates = crate::CrateItemQuery::new(&def_maps, &items, use_site)
            .visible_stores()
            .expect("visible semantic stores should load")
            .into_iter()
            .map(crate::ItemStore::crate_ref)
            .collect::<Vec<_>>();
        crates.sort_by_key(|crate_ref| (crate_ref.package.0, crate_ref.crate_id.0));
        crates
    };
    let sorted = |mut crates: Vec<rg_ir_model::CrateRef>| {
        crates.sort_by_key(|crate_ref| (crate_ref.package.0, crate_ref.crate_id.0));
        crates
    };

    assert_eq!(
        visible_from(app),
        sorted(vec![app, runtime]),
        "a consumer should see its runtime dependency, but neither a proc-macro implementation nor its host dependencies",
    );
    assert_eq!(
        visible_from(derive_macro),
        sorted(vec![derive_macro, parser]),
        "a proc-macro crate should see its own implementation and normal host dependencies",
    );
}

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

pub struct UsesAbsolute(::semantic_fixture::UserId);

pub type UserResult<T> = Result<User<T>, DbError>;
pub type AbsoluteAlias = ::semantic_fixture::UserId;
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
            - pub struct UsesAbsolute
              - field #0: ::semantic_fixture::UserId
            - pub type UserResult<T> = Result<User<T>, DbError>
            - pub type AbsoluteAlias = ::semantic_fixture::UserId
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
fn lowers_source_and_generated_foreign_declaration_signatures() {
    check_project_semantic_ir(
        r#"
//- /Cargo.toml
[package]
name = "foreign_semantic_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
extern "C" {
    pub fn foreign_fn(input: u32) -> u64;
    pub static mut FOREIGN_STATIC: u32;
    pub type Opaque;
}

macro_rules! make_foreign {
    () => {
        extern "C" {
            pub fn generated_fn(input: u8) -> u16;
            pub static GENERATED_STATIC: u8;
            pub type GeneratedOpaque;
        }
    };
}

make_foreign!();
"#,
        expect![[r#"
            package foreign_semantic_fixture

            foreign_semantic_fixture [lib]
            crate
            - pub fn foreign_fn(input: u32) -> u64
            - pub static mut FOREIGN_STATIC: u32
            - pub type Opaque
            - pub fn generated_fn(input: u8) -> u16
            - pub static GENERATED_STATIC: u8
            - pub type GeneratedOpaque
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
fn resolves_cross_crate_impl_queries_from_root_and_module_scopes() {
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

pub mod api {
    pub struct ScopedLocal;

    impl ScopedLocal {
        pub fn local_method(&self) {}
    }

    impl crate::ImportedTrait for ScopedLocal {
        fn required(&self) {}
    }
}

mod consumer {
    use crate::api::ScopedLocal as ImportedScoped;
}
"#,
        &[
            SemanticQuery::lib("app", "Local"),
            SemanticQuery::lib_from("app", "crate::consumer", "ImportedScoped"),
        ],
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


            query app [lib] crate::consumer resolves ImportedScoped -> struct app[lib]::crate::api::ScopedLocal
            impls
            - impl ScopedLocal
            - impl crate::ImportedTrait for ScopedLocal
            trait impls
            - impl crate::ImportedTrait for ScopedLocal => trait dep[lib]::crate::ExternalTrait
            traits
            - trait dep[lib]::crate::ExternalTrait
            inherent functions
            - fn impl ScopedLocal::local_method
            trait functions
            - fn trait dep[lib]::crate::ExternalTrait::defaulted
            - fn trait dep[lib]::crate::ExternalTrait::required
            trait impl functions
            - fn impl crate::ImportedTrait for ScopedLocal::required
        "#]],
    );
}

#[test]
fn resolves_core_and_alloc_impl_headers_through_core_prelude() {
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
pub struct AllocType;

impl Marker for AllocType {
    fn mark(&self) {}
}

//- /sysroot/library/std/src/lib.rs
pub mod prelude {
    pub mod rust_2024 {}
}

//- /sysroot/library/proc_macro/src/lib.rs
pub struct TokenStream;
"#,
        &[
            SemanticQuery::lib("core", "CoreType"),
            SemanticQuery::lib("alloc", "AllocType"),
        ],
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
fn crate_queries_exclude_impls_from_unrelated_workspace_crates() {
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
