std::cfg_select! {
    target_os = "linux" => {
        pub mod shared;

        use gungraun::{
            LibraryBenchmarkConfig, library_benchmark, library_benchmark_group,
            main as gungraun_main,
        };

        use self::shared::query::{BenchQuery, PreparedQuery};

        fn setup_query(query: BenchQuery) -> PreparedQuery {
            PreparedQuery::new(query)
        }

        // Gungraun's library-benchmark setup hook runs before Callgrind measures the function
        // body. That keeps project construction, cache loading, path lookup, and marker resolution
        // out of the instruction counts while still measuring the same PreparedQuery runner as the
        // Divan benchmark.
        #[library_benchmark(setup = setup_query)]
        #[bench::small_app_hover_workspace_summary(BenchQuery::HoverWorkspaceSummary)]
        #[bench::small_app_goto_workspace_constructor(BenchQuery::GotoWorkspaceConstructor)]
        #[bench::small_app_references_workspace_summary(BenchQuery::ReferencesWorkspaceSummary)]
        #[bench::small_app_document_highlight_summary(BenchQuery::DocumentHighlightSummary)]
        #[bench::small_app_completion_workspace_summary(BenchQuery::CompletionWorkspaceSummary)]
        #[bench::small_app_workspace_symbols_workspace(BenchQuery::WorkspaceSymbolsWorkspace)]
        fn frozen_project_query(prepared: PreparedQuery) -> usize {
            std::hint::black_box(prepared.run())
        }

        library_benchmark_group!(
            name = query_benches;
            benchmarks = frozen_project_query
        );

        gungraun_main!(
            // Query setup shells out through cargo metadata/fetch before measurement, so keep the normal
            // development environment intact while Callgrind ignores child processes.
            config = LibraryBenchmarkConfig::default()
                .env_clear(false)
                .valgrind_args(["--trace-children=no"]);
            library_benchmark_groups = query_benches
        );
    }

    _ => {
        fn main() {
            eprintln!("query_callgrind is only collected on Linux because Gungraun uses Callgrind");
        }
    }
}
