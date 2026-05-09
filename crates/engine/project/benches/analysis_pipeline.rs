mod shared;

use divan::{
    Bencher, black_box, black_box_drop,
    counter::{BytesCount, ItemsCount},
};
use rg_body_ir::{BodyIrBuildPolicy, BodyIrDb};
use rg_def_map::DefMapDb;
use rg_item_tree::ItemTreeDb;
use rg_parse::ParseDb;
use rg_semantic_ir::SemanticIrDb;
use rg_text::PackageNameInterners;

use self::shared::{BenchFixture, BenchTarget, bench_targets};

fn main() {
    divan::main();
}

#[divan::bench(args = bench_targets(), sample_count = 10, sample_size = 1)]
fn parse_db(bencher: Bencher<'_, '_>, target: BenchTarget) {
    let fixture = BenchFixture::get(target);
    bencher
        .counter(BytesCount::from(fixture.source_bytes))
        .counter(ItemsCount::new(fixture.source_files))
        .bench_local(|| {
            let parse =
                ParseDb::build(black_box(&fixture.workspace)).expect("parse db should build");
            black_box_drop(parse);
        });
}

#[divan::bench(args = bench_targets(), sample_count = 10, sample_size = 1)]
fn item_tree_db(bencher: Bencher<'_, '_>, target: BenchTarget) {
    let fixture = BenchFixture::get(target);
    bencher
        .counter(ItemsCount::new(fixture.item_tree_items))
        .with_inputs(|| {
            (
                fixture.parse.clone(),
                PackageNameInterners::new(fixture.parse.package_count()),
            )
        })
        .bench_local_values(|(mut parse, mut names)| {
            let item_tree = ItemTreeDb::build_with_interners(&mut parse, &mut names)
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
            (
                fixture.parse.clone(),
                fixture.item_tree.clone(),
                fixture.names_after_item_tree.clone(),
            )
        })
        .bench_local_values(|(parse, item_tree, mut names)| {
            let def_map = DefMapDb::builder(&fixture.workspace, &parse, &item_tree)
                .name_interners(&mut names)
                .build()
                .expect("def map should build");
            black_box_drop(def_map);
        });
}

#[divan::bench(args = bench_targets(), sample_count = 10, sample_size = 1)]
fn semantic_ir_db(bencher: Bencher<'_, '_>, target: BenchTarget) {
    let fixture = BenchFixture::get(target);
    bencher
        .counter(ItemsCount::new(fixture.semantic_items))
        .with_inputs(|| (fixture.item_tree.clone(), fixture.def_map.clone()))
        .bench_local_values(|(item_tree, def_map)| {
            let semantic_ir = SemanticIrDb::builder(&item_tree, &def_map)
                .build()
                .expect("semantic IR should build");
            black_box_drop(semantic_ir);
        });
}

#[divan::bench(args = bench_targets(), sample_count = 10, sample_size = 1)]
fn body_ir_db(bencher: Bencher<'_, '_>, target: BenchTarget) {
    let fixture = BenchFixture::get(target);
    bencher
        .counter(ItemsCount::new(fixture.body_expressions))
        .with_inputs(|| {
            (
                fixture.parse.clone(),
                fixture.names_after_semantic_ir.clone(),
            )
        })
        .bench_local_values(|(parse, mut names)| {
            let body_ir = BodyIrDb::builder(&parse, &fixture.def_map, &fixture.semantic_ir)
                .name_interners(&mut names)
                .policy(BodyIrBuildPolicy::workspace_packages())
                .build()
                .expect("body IR should build");
            black_box_drop(body_ir);
        });
}
