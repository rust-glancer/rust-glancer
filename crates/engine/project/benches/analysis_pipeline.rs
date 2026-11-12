//! Keep these phase names in this file so CodSpeed can compare them with the existing series.
//! Each sample uses a fresh in-memory database. Earlier phases are prepared outside the timer.

mod shared;

use divan::{Bencher, black_box_drop};
use rg_body_ir::saved_body;

use self::shared::{BenchTarget, BenchWorkspace, bench_targets};

fn main() {
    divan::main();
}

#[divan::bench(args = bench_targets(), sample_count = 10, sample_size = 1)]
fn parse_db(bencher: Bencher<'_, '_>, target: BenchTarget) {
    let workspace = BenchWorkspace::load(target);
    bencher
        .with_inputs(|| workspace.database())
        .bench_local_refs(|database| {
            let inputs = database.inputs().clone();
            let parse = database
                .run(|db| workspace.parse_roots(db, &inputs))
                .expect("target roots should parse");
            black_box_drop(parse);
        });
}

#[divan::bench(args = bench_targets(), sample_count = 10, sample_size = 1)]
fn item_tree_db(bencher: Bencher<'_, '_>, target: BenchTarget) {
    let workspace = BenchWorkspace::load(target);
    bencher
        .with_inputs(|| {
            let mut database = workspace.database();
            let inputs = database.inputs().clone();
            database
                .run(|db| workspace.parse_roots(db, &inputs))
                .expect("capture target roots");
            database
        })
        .bench_local_refs(|database| {
            // Root text is captured, but syntax is transient. Discovery reparses it just as an
            // independent Ajika operation would.
            let trees = database
                .run(|db| workspace.item_trees(db))
                .expect("item trees should build");
            black_box_drop(trees);
        });
}

#[divan::bench(args = bench_targets(), sample_count = 10, sample_size = 1)]
fn def_map_db(bencher: Bencher<'_, '_>, target: BenchTarget) {
    let workspace = BenchWorkspace::load(target);
    bencher
        .with_inputs(|| {
            let mut database = workspace.database();
            database
                .run(|db| workspace.item_trees(db))
                .expect("prepare ordinary item trees");
            database
        })
        .bench_local_refs(|database| {
            let scopes = database
                .run(|db| workspace.scopes(db))
                .expect("definition scopes should build");
            black_box_drop(scopes);
        });
}

#[divan::bench(args = bench_targets(), sample_count = 10, sample_size = 1)]
fn semantic_ir_db(bencher: Bencher<'_, '_>, target: BenchTarget) {
    let workspace = BenchWorkspace::load(target);
    bencher
        .with_inputs(|| {
            let mut database = workspace.database();
            let scopes = database
                .run(|db| workspace.scopes(db))
                .expect("prepare definition scopes");
            (database, scopes)
        })
        .bench_local_values(|(mut database, scopes)| {
            let declarations = database
                .run(|db| scopes.build_items(db))
                .expect("semantic items and lookup indexes should build");
            black_box_drop(declarations);
            database
        });
}

#[divan::bench(args = bench_targets(), sample_count = 10, sample_size = 1)]
fn body_ir_db(bencher: Bencher<'_, '_>, target: BenchTarget) {
    let workspace = BenchWorkspace::load(target);
    bencher
        .with_inputs(|| {
            let mut database = workspace.database();
            let owners = database
                .run(|db| workspace.body_owners(db))
                .expect("prepare declarations and select primary workspace bodies");
            (database, owners)
        })
        .bench_local_refs(|(database, owners)| {
            let bodies = database
                .run(|db| db.parallel_map(owners, |db, &key| db.query::<saved_body>(key)))
                .expect("saved bodies should build");
            black_box_drop(bodies);
        });
}
