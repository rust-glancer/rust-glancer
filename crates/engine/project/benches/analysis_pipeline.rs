mod shared;

use divan::{
    Bencher, black_box, black_box_drop,
    counter::{BytesCount, ItemsCount},
};
use rg_parse::ParseDb;

use self::shared::{BenchFixture, BenchTarget, bench_targets};

fn main() {
    divan::main();
}

#[divan::bench(args = bench_targets(), sample_count = 10, sample_size = 1)]
fn parse_db(bencher: Bencher<'_, '_>, target: BenchTarget) {
    let fixture = BenchFixture::get(target);
    bencher
        .counter(BytesCount::new(fixture.source_bytes))
        .counter(ItemsCount::new(fixture.source_files))
        .bench_local(|| {
            let parse = ParseDb::build(black_box(&fixture.workspace), Default::default())
                .expect("parse db should build");
            black_box_drop(parse);
        });
}

#[divan::bench(args = bench_targets(), sample_count = 10, sample_size = 1)]
fn item_tree_db(bencher: Bencher<'_, '_>, target: BenchTarget) {
    let fixture = BenchFixture::get(target);
    bencher
        .counter(ItemsCount::new(fixture.item_tree_items))
        .with_inputs(|| {
            let phases = fixture.file_phases();
            phases.reset_queries(&phases.workspace_inputs);
            let parse = phases.parse.clone();
            (phases, parse)
        })
        .bench_local_values(|(phases, mut parse)| {
            let item_tree = rg_project::bench_support::build_item_tree(
                &mut phases.database.borrow_mut(),
                &mut parse,
            )
            .expect("item tree should build");
            black_box_drop(item_tree);
        });
}

#[divan::bench(args = bench_targets(), sample_count = 10, sample_size = 1)]
fn def_map_db(bencher: Bencher<'_, '_>, target: BenchTarget) {
    let fixture = BenchFixture::get(target);
    bencher
        .counter(ItemsCount::new(fixture.def_map_imports))
        .with_inputs(|| {
            let phases = fixture.file_phases();
            phases.reset_queries(&phases.def_map_inputs);
            let parse = phases.parse_before_def_map.clone();
            let item_tree = phases.item_tree_before_def_map.clone();
            (phases, parse, item_tree)
        })
        .bench_local_values(|(phases, mut parse, mut item_tree)| {
            let def_map = rg_project::bench_support::build_def_map(
                &mut phases.database.borrow_mut(),
                &fixture.workspace,
                &mut parse,
                &mut item_tree,
            )
            .expect("def map should build");
            black_box_drop(def_map);
        });
}

#[divan::bench(args = bench_targets(), sample_count = 10, sample_size = 1)]
fn semantic_ir_db(bencher: Bencher<'_, '_>, target: BenchTarget) {
    let fixture = BenchFixture::get(target);
    bencher
        .counter(ItemsCount::new(fixture.semantic_items))
        .with_inputs(|| {
            (
                fixture.item_tree_after_def_map.clone(),
                fixture.def_map.clone(),
                fixture.generated_items.clone(),
            )
        })
        .bench_local_values(|(item_tree, def_map, generated_items)| {
            let semantic_ir = rg_project::bench_support::build_semantic_ir(
                &item_tree,
                &def_map,
                &generated_items,
            )
            .expect("semantic IR should build");
            black_box_drop(semantic_ir);
        });
}

#[divan::bench(args = bench_targets(), sample_count = 10, sample_size = 1)]
fn body_ir_db(bencher: Bencher<'_, '_>, target: BenchTarget) {
    let fixture = BenchFixture::get(target);
    bencher
        .counter(ItemsCount::new(fixture.body_expressions))
        .with_inputs(|| fixture.parse_after_def_map.clone())
        .bench_local_values(|parse| {
            let body_ir = rg_project::bench_support::build_body_ir(
                &parse,
                &fixture.def_map,
                &fixture.semantic_ir,
            )
            .expect("body IR should build");
            black_box_drop(body_ir);
        });
}
