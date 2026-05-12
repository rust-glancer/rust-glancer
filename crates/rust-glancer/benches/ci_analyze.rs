#[cfg(target_os = "linux")]
use std::path::PathBuf;

#[cfg(target_os = "linux")]
use gungraun::{
    BinaryBenchmarkConfig, binary_benchmark, binary_benchmark_group, main as gungraun_main,
};

#[cfg(target_os = "linux")]
#[binary_benchmark]
#[bench::moderate_workspace()]
fn ci_analyze() -> gungraun::Command {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test_targets/moderate_workspace");

    gungraun::Command::new(env!("CARGO_BIN_EXE_rust-glancer"))
        .arg("analyze")
        .arg(fixture)
        .args(["--package-residency", "all-resident"])
        .build()
}

#[cfg(target_os = "linux")]
binary_benchmark_group!(
    name = rust_glancer_analyze;
    benchmarks = ci_analyze
);

#[cfg(target_os = "linux")]
gungraun_main!(
    // `analyze` shells out through cargo_metadata, so keep PATH/rustup/Cargo
    // environment intact while Callgrind itself ignores child processes.
    config = BinaryBenchmarkConfig::default()
        .env_clear(false)
        .valgrind_args(["--trace-children=no"]);
    binary_benchmark_groups = rust_glancer_analyze
);

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("ci_analyze is only collected on Linux because Gungraun uses Callgrind");
}
