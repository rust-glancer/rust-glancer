pub mod shared;

use divan::{Bencher, black_box, black_box_drop};

use self::shared::query::{BenchQuery, PreparedQuery, divan_queries};

fn main() {
    divan::main();
}

// Divan gives us wall-clock timings for day-to-day local iteration. `with_inputs` prepares the
// frozen project query outside the timed loop; `bench_local_values` then measures one complete
// project-layer query run and consumes the result count so the optimizer cannot erase the work.
#[divan::bench(args = divan_queries(), sample_count = 10, sample_size = 1)]
fn frozen_project_query(bencher: Bencher<'_, '_>, query: BenchQuery) {
    bencher
        .with_inputs(|| PreparedQuery::new(query))
        .bench_local_values(|prepared| {
            let result_count = prepared.run();
            black_box_drop(black_box(result_count));
        });
}
