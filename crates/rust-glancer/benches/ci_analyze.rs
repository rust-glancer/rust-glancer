std::cfg_select! {
    target_os = "linux" => {
        use std::path::PathBuf;

        use gungraun::{
            BinaryBenchmarkConfig, binary_benchmark, binary_benchmark_group, main as gungraun_main,
        };

        // This binary benchmark measures the CLI-facing project build path end to end. It stays as
        // a binary benchmark because process startup and argument parsing are part of the CI
        // `rust-glancer analyze` scenario we want to track.
        #[binary_benchmark]
        #[bench::moderate_workspace()]
        fn ci_analyze() -> gungraun::Command {
            let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../test_targets/moderate_workspace");

            gungraun::Command::new(env!("CARGO_BIN_EXE_rust-glancer"))
                .arg("analyze")
                .arg(fixture)
                .args(["--package-residency", "all-resident"])
                .build()
        }

        binary_benchmark_group!(
            name = rust_glancer_analyze;
            benchmarks = ci_analyze
        );

        gungraun_main!(
            // `analyze` shells out through cargo_metadata, so keep PATH/rustup/Cargo
            // environment intact while Callgrind itself ignores child processes.
            config = BinaryBenchmarkConfig::default()
                .env_clear(false)
                .valgrind_args(["--trace-children=no"]);
            binary_benchmark_groups = rust_glancer_analyze
        );
    }

    _ => {
        fn main() {
            eprintln!("ci_analyze is only collected on Linux because Gungraun uses Callgrind");
        }
    }
}
