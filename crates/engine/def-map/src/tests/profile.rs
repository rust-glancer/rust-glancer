use crate::{profile::metric, profile_descriptors};

use super::utils;

fn macro_fixture() -> &'static str {
    r#"
//- /Cargo.toml
[package]
name = "def_map_profile_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_item {
    ($name:ident) => {
        pub struct $name;
    };
}

make_item!(User);
make_item!(Admin);
"#
}

#[test]
fn profile_snapshot_records_macro_finalization_metrics() {
    let run = rg_profile::test_support::ProfileTest::start(
        profile_descriptors(),
        "def_map.finalization,def_map.macros.by_name",
    );

    let project = utils::DefMapFixtureDb::build(macro_fixture());
    let snapshot = run.finish();
    let target = project.lib("def_map_profile_fixture");

    target
        .entry("User")
        .assert_type_exists("profile collection should not change def-map output");

    snapshot.assert_counter_with_message(
        metric::MACRO_CALLS_EXPANDED,
        2,
        "the fixture should expand both macro calls",
    );
    snapshot.assert_counter_with_message(
        metric::MACRO_COMPILE_ATTEMPTS,
        1,
        "multiple calls to one macro definition should share compiled macro data",
    );
    snapshot.assert_counter_with_message(
        metric::MACRO_COMPILE_CACHE_HITS,
        1,
        "the second call should reuse the cached compiled macro",
    );
    snapshot.assert_counter_with_message(
        metric::GENERATED_SOURCES_PARSED,
        2,
        "each expanded generated item source should be parsed",
    );
    snapshot.assert_keyed_duration_count_with_message(
        metric::EXPANSION_BY_NAME,
        "make_item",
        2,
        "the profile should preserve by-macro expansion counts",
    );
    snapshot.assert_gauge_count_with_message(
        metric::EXPANSION_PASS_LIMIT,
        128,
        "finalization gauges should be available in the snapshot",
    );
}

#[test]
fn filtered_profile_records_requested_macro_scope() {
    let run = rg_profile::test_support::ProfileTest::start(
        profile_descriptors(),
        "def_map.macros.by_name",
    );

    let project = utils::DefMapFixtureDb::build(macro_fixture());
    let snapshot = run.finish();
    let target = project.lib("def_map_profile_fixture");

    target
        .entry("User")
        .assert_type_exists("profile collection should not change def-map output");
    snapshot.assert_counter_with_message(
        metric::MACRO_CALLS_EXPANDED,
        2,
        "profile collection should not depend on retaining legacy stats",
    );
}
