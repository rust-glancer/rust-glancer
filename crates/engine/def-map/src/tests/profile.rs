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

#[test]
fn profile_snapshot_records_import_pass_work_and_changes() {
    let run =
        rg_profile::test_support::ProfileTest::start(profile_descriptors(), "def_map.finalization");

    // `source -> bridge -> root` takes several import waves. The unrelated branch settles without
    // feeding the chain, so later checkpoints should show that its import group was not rerun.
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /Cargo.toml
[package]
name = "def_map_import_profile_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod source {
    pub struct User;
}

mod bridge {
    pub use crate::source::*;
}

mod unrelated_source {
    pub struct Unrelated;
}

mod unrelated_importer {
    pub use crate::unrelated_source::*;
}

use bridge::*;
"#,
    );
    let snapshot = run.finish();

    project
        .lib("def_map_import_profile_fixture")
        .entry("User")
        .assert_type_exists("profile collection should not change glob import output");

    let passes = snapshot
        .inner()
        .checkpoints(metric::IMPORT_RESOLUTION_PASS_CHECKPOINTS.path())
        .expect("import resolution pass checkpoints should be recorded");
    assert!(
        passes.len() >= 2,
        "one changing pass and one stable pass should be observed"
    );
    assert_eq!(
        checkpoint_count(&passes[0], "imports_evaluated"),
        3,
        "the first wave should evaluate every module import group"
    );
    assert_eq!(
        checkpoint_count(&passes[0], "glob_imports_evaluated"),
        3,
        "all fixture imports should be classified as globs"
    );
    assert!(
        checkpoint_count(&passes[0], "glob_bindings_emitted") > 0,
        "the glob should expose at least one source binding"
    );
    let initially_evaluated_modules = checkpoint_count(&passes[0], "evaluated_modules");
    assert!(
        passes.iter().skip(1).any(|pass| {
            checkpoint_count(pass, "evaluated_modules") < initially_evaluated_modules
        }),
        "later waves should skip the unrelated import module once its inputs settle"
    );
    assert!(
        passes
            .iter()
            .any(|pass| checkpoint_count(pass, "changed_modules") == 0),
        "the final checkpoint should describe the stable pass"
    );
}

fn checkpoint_count(checkpoint: &rg_profile::ProfileCheckpoint, key: &str) -> u64 {
    let value = checkpoint
        .values
        .iter()
        .find(|value| value.key == key)
        .unwrap_or_else(|| panic!("checkpoint should include {key:?}"));
    match value.value {
        rg_profile::ProfileMeasurement::Count(count) => count,
        ref value => panic!("checkpoint value {key:?} should be a count, got {value:?}"),
    }
}
