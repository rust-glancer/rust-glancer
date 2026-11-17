mod builtin_derives;
mod item_reads;
mod trait_impl_lookup;
mod utils;

use std::fmt::Write as _;

use expect_test::expect;
use rg_ir_model::{CrateId, CrateRef, DefMapRef, LocalDefRef, SemanticItemRef};
use rg_std::UniqueVec;
use rg_workspace::TargetKind;

use self::utils::{
    SemanticQuery, check_project_semantic_ir, check_project_semantic_queries,
    check_project_semantic_queries_with_sysroot,
};
use crate::{CrateItemSource as _, testonly::SemanticIrFixture};

#[test]
fn indexed_lookup_facts_keep_language_items_per_use_site() {
    let mut fixture = SemanticIrFixture::build(
        r#"
//- /dep/src/lib.rs crate:dep root:dep
#[lang = "deref"]
pub trait DependencyDeref {}

//- /app/src/lib.rs crate:app root:app deps:dep
#[lang = "deref"]
pub trait LibraryDeref {}

//- /app/src/main.rs crate:app root:app deps:dep kind:bin
#[lang = "deref"]
trait BinaryDeref {}

fn main() {}
"#,
    );
    fixture.inspect(|view| {
        let [library, dependency, binary] = [
            ("app", TargetKind::Lib),
            ("dep", TargetKind::Lib),
            ("app", TargetKind::Bin),
        ]
        .map(|(package, kind)| view.crate_ref(package, kind));
        let items = view.items();
        let cache = crate::ItemLookupQueryCache::new();
        let cancellation = rg_std::CancellationToken::new();

        // Each target's local language item wins over dependencies. Interleaving repeated queries
        // also checks that sharing the query cache does not mix library and binary choices.
        for use_site in [library, dependency, binary, binary, dependency, library] {
            let expected = view
                .all_items(DefMapRef::Crate(use_site))
                .iter()
                .find_map(|item| match item.item() {
                    SemanticItemRef::Trait(trait_ref) => Some(trait_ref),
                    _ => None,
                })
                .expect("target declares its language item");
            let query =
                crate::ItemLookupQuery::build_with_cache(items, use_site, &cache, &cancellation)
                    .expect("target lookup succeeds");
            assert_eq!(
                query.lang_trait(rg_item_tree::LangItem::Deref),
                Some(expected),
                "wrong language item for {use_site:?}",
            );
        }
    });
}

#[test]
fn high_target_fanout_shares_results_by_ordered_dependency_set() {
    let mut retained_entries = Vec::new();

    for target_count in [1_usize, 4, 12] {
        let mut test_sources = String::new();
        for target_idx in 0..target_count {
            writeln!(
                test_sources,
                r#"
//- /app/tests/test_{target_idx}.rs crate:app root:app dev-deps:dep kind:test name:test-{target_idx}
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
//- /dep/src/lib.rs crate:dep root:dep
pub trait Shared {{
    fn shared(&self);
}}

pub struct SharedType;

impl Shared for SharedType {{
    fn shared(&self) {{}}
}}

//- /app/src/lib.rs crate:app root:app dev-deps:dep
pub struct Library;
{test_sources}
"#,
        );
        let mut fixture = SemanticIrFixture::build(&fixture_source);
        fixture.inspect(|view| {
            let dep = view.crate_ref("dep", TargetKind::Lib);
            let dependency_items = view.all_items(DefMapRef::Crate(dep));
            let shared_trait = dependency_items.iter().find_map(|item| match item.item() {
                SemanticItemRef::Trait(trait_ref) => Some(trait_ref),
                _ => None,
            }).expect("dependency should declare one trait");
            let shared_type = dependency_items.iter().find_map(|item| {
                (item.name()?.as_str() == "SharedType").then(|| item.type_def()).flatten()
            }).expect("dependency should declare SharedType");
            let app = view.package_slot_by_name("app");
            let test_crates = view.parse_db().packages()[app.0]
                .targets()
                .iter()
                .filter(|target| target.kind == TargetKind::Test)
                .map(|target| CrateRef {
                    package: app,
                    crate_id: CrateId(target.id.0),
                })
                .collect::<Vec<_>>();

            let items = view.items();
            let cache = crate::ItemLookupQueryCache::new();
            let dependency_sets = test_crates
                .iter()
                .map(|&use_site| {
                    items
                        .lookup_index(use_site)
                        .expect("test target lookup index should load")
                        .expect("test target lookup index should exist")
                        .lookup_facts()
                        .visible_crates()
                        .iter()
                        .copied()
                        .filter(|&crate_ref| crate_ref != use_site)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            let local_types = test_crates
                .iter()
                .map(|&use_site| {
                    items
                        .all_items(DefMapRef::Crate(use_site))
                        .expect("test target items should load")
                        .into_iter()
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
                        items,
                        use_site,
                        &cache,
                        &rg_std::CancellationToken::new(),
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
                    .expect("candidate lookup succeeds")
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
                let local_impls = query
                    .trait_impls_for_type(*local_type)
                    .expect("candidate lookup succeeds");
                let local_impl = local_impls
                    .as_one()
                    .expect("each test target should find exactly its own local trait impl");
                assert_eq!(local_impl.trait_ref, shared_trait);
                assert_eq!(local_impl.impl_ref.origin.as_crate_ref(), Some(*use_site));
            }
            if queries.len() > 1 {
                assert!(
                    queries[1]
                        .trait_impls_for_type(local_types[0])
                        .expect("candidate lookup succeeds")
                        .is_empty(),
                    "a shared dependency cache must not expose another target's local impl",
                );
            }

            let index_source = view.declarations();
            let crates = index_source.crates();
            assert_eq!(crates.len(), target_count + 2);
            retained_entries.push(
                crates
                    .into_iter()
                    .map(|krate| {
                        index_source
                            .lookup_index(krate)
                            .expect("fixture lookup index reads")
                            .expect("fixture target has a lookup index")
                            .entry_count()
                    })
                    .sum::<usize>(),
            );
        });
    }

    assert_eq!(
        (retained_entries[1] - retained_entries[0]) * 8,
        (retained_entries[2] - retained_entries[1]) * 3,
        "retained local-index entries should grow linearly with target-local declarations",
    );
}

#[test]
fn proc_macro_exports_link_to_semantic_implementation_functions() {
    let mut fixture = SemanticIrFixture::build(
        r#"
//- /src/lib.rs crate:semantic_proc_macros kind:proc-macro
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
    fixture.inspect(|view| {
        let crate_ref = view.crate_ref("semantic_proc_macros", TargetKind::ProcMacro);
        let def_map = view
            .def_map(crate_ref)
            .expect("proc-macro def map should exist");
        let items = view.all_items(DefMapRef::Crate(crate_ref));

        let mut function_names = items
            .iter()
            .filter_map(|item| item.function_data())
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
                view.items()
                    .item_for_local_def(LocalDefRef {
                        origin: DefMapRef::Crate(crate_ref),
                        local_def: export.local_def
                    })
                    .expect("fixture definition should load")
                    .is_none(),
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

            let Some(SemanticItemRef::Function(function)) = view
                .items()
                .item_for_local_def(LocalDefRef {
                    origin: DefMapRef::Crate(crate_ref),
                    local_def: implementation,
                })
                .expect("fixture definition should load")
            else {
                panic!(
                    "proc-macro implementation `{implementation_name}` should lower as a function"
                );
            };
            assert_eq!(
                crate::ItemStoreQuery::new(&view.items())
                    .function_data(function)
                    .expect("fixture function should load")
                    .expect("proc-macro implementation function should exist")
                    .name
                    .as_str(),
                implementation_name,
            );
        }
    });
}

#[test]
fn proc_macro_implementation_stores_do_not_cross_the_dependency_boundary() {
    let mut fixture = SemanticIrFixture::build(
        r#"
//- /app/src/lib.rs crate:app root:app deps:runtime
pub struct App;

//- /runtime/src/lib.rs crate:runtime root:runtime deps:derive_macro
pub struct Runtime;

//- /derive_macro/src/lib.rs crate:derive_macro root:derive_macro deps:parser kind:proc-macro
extern crate proc_macro;

struct Implementation;

impl Implementation {
    fn host_only() {}
}

#[proc_macro]
pub fn emit(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    input
}

//- /parser/src/lib.rs crate:parser root:parser
pub struct Parser;
"#,
    );
    fixture.inspect(|view| {
        let app = view.crate_ref("app", TargetKind::Lib);
        let runtime = view.crate_ref("runtime", TargetKind::Lib);
        let derive_macro = view.crate_ref("derive_macro", TargetKind::ProcMacro);
        let parser = view.crate_ref("parser", TargetKind::Lib);
        let items = view.items();
        let visible_from = |use_site| {
            let mut crates = items
                .lookup_index(use_site)
                .expect("target lookup index should load")
                .expect("target lookup index should exist")
                .lookup_facts()
                .visible_crates()
                .as_slice()
                .to_vec();
            crates.sort_by_key(|crate_ref| (crate_ref.package.0, crate_ref.crate_id.0));
            crates
        };
        let sorted = |mut crates: Vec<CrateRef>| {
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
    });
}

#[test]
fn dumps_semantic_ir_signatures() {
    check_project_semantic_ir(
        r#"
//- /src/lib.rs crate:semantic_fixture
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
//- /src/lib.rs crate:foreign_semantic_fixture
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
//- /src/lib.rs crate:generated_semantic_fixture
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
//- /crates/dep/src/lib.rs crate:dep root:crates/dep
pub trait ExternalTrait {
    fn required(&self);
    fn defaulted(&self) {}
}

//- /crates/app/src/lib.rs crate:app root:crates/app deps:dep
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
//- /src/lib.rs crate:app
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
//- /crates/shared/src/lib.rs crate:shared root:crates/shared
pub struct Maybe;

impl Maybe {
    pub fn is_some(&self) -> bool {
        true
    }
}

//- /crates/app/src/lib.rs crate:app root:crates/app deps:shared
use shared::Maybe;

pub trait AppExt {
    fn and_then(&self);
}

impl AppExt for Maybe {
    fn and_then(&self) {}
}

//- /crates/other/src/lib.rs crate:other root:crates/other deps:shared
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
//- /crates/dep/src/lib.rs crate:dep root:crates/dep
pub struct Thing;

impl Thing {
    pub fn external(&self) {}
}

//- /crates/app/src/lib.rs crate:app root:crates/app deps:dep
pub struct Api;

impl Api {
    pub fn local(&self) {}
}

//- /crates/app/src/main.rs crate:app root:crates/app deps:dep kind:bin name:app-bin
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
