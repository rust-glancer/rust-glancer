---
name: rust-glancer-debugging
description: Debug rust-glancer through its repo-owned bounded runner. Use when investigating analysis, indexing, memory or peak-RSS behavior; comparing rust-glancer with rust-analyzer; querying hover or inlay hints through the real LSP; reproducing hangs; running focused tests with custom environment or logging; or creating an ad-hoc Cargo fixture.
---

# Rust Glancer Debugging

Use `just agent-debug` from the workspace root for the common debugging path. Let the runner build
the current binary, preserve argv boundaries, record artifacts, enforce a per-run timeout, and
clean up its complete process group. The runner supports macOS and Linux.

## Choose the workflow

- Use `analyze` for indexing, profiling, residency, cache, and memory questions.
- Use `compare-lsp` for the established rust-analyzer comparison fixture.
- Use `lsp-query` for real hover or inlay results from rust-glancer.
- Use `test` for focused `cargo nextest run` arguments plus managed environment and timeout.
- Use `fixture init <name>` for an ad-hoc Cargo project under `target/agent-debug/fixtures`.

List reusable fixtures with `just agent-debug fixture list`, and print a fixture's absolute path
with `just agent-debug fixture path <name>`. These administrative modes do not accept runner
options. Use `--no-build` before a managed mode when the runner's existing host-target binary is
intentionally sufficient.

Read `just agent-debug --help` before constructing an unusual invocation. Runner options go before
the mode; every argument after the mode is forwarded literally to that mode. `--timeout` applies
to each warm-up and measured run. The Cargo build has a separate fixed 20-minute timeout.
Test mode inherits the system temporary directory so tempfile-backed Cargo fixtures remain outside
the rust-glancer workspace. Other runtime modes keep scratch files in their managed run directory.
Use `--env TMPDIR=<path>` before the mode only when a test intentionally needs an override.

```bash
just agent-debug --log 'rg_lsp_engine=debug' analyze . --profile default,macros -m
just agent-debug --measure analyze path/to/rust-analyzer --profile --package-residency all-offloadable -m
just agent-debug --timeout 60s --sample-on-timeout test -p rg_analysis inference_test
```

## Query the LSP

Pass a single query directly. Marker text, including spaces or punctuation, is safe as one argument.

```bash
just agent-debug lsp-query \
  --file crates/engine/analysis/src/lib.rs \
  hover --marker 'let result' --delta 4
```

For several queries, use `--query-json` or a plan under `target/agent-debug/queries`. A plan may
carry inline source in `text`, so an unsaved-buffer check does not need another temporary file. The
default readiness barrier waits until the workspace is queryable; use `deferredBarrier` only when
the result specifically needs to run before or after deferred indexing. Run
`just agent-debug lsp-query --help` for the bounded plan shape.

Use `--workspace-root target/agent-debug/fixtures/<name>` when querying an ad-hoc fixture. `--file`
is resolved relative to that workspace root. Do not write a one-off JSON-RPC client.

## Add diagnostics without changing the command shape

- Use `--log <filter>` instead of prefixing `RUST_GLANCER_LOG`. LSP server logs are retained as
  `lsp-server.stderr.log` in the numbered run directory without flooding successful query output.
- Use `--env NAME=VALUE`, `--backtrace`, or `--full-backtrace` for runtime diagnostics.
- Use `--measure` instead of shell `time`; read parsed peak RSS with `just agent-debug last`.
- Use `--warmup` and `--repeat` instead of a shell loop.
- Use `--isolated-cache [name]` only when cache isolation is part of the experiment. Normal analysis
  intentionally keeps the analyzed workspace's usual Cargo target behavior.

## Handle hangs

Keep managed commands in the foreground. On a suspected hang, let the runner reach its deadline or
interrupt its execution session. It records the owned process group, optionally samples the stuck
rust-glancer engine on macOS, sends a bounded graceful termination, then force-kills survivors.
An active numbered run also contains `process.json`, so inspect that artifact instead of discovering
PIDs with a separate process-list command. The result's `cleanup.verifiedEmpty` field and the
top-level `processCleanup` summary record whether the owned groups were observed empty after each
run; `just agent-debug last` prints the aggregate status.

Do not use `ps`, `pgrep`, `pkill`, or `kill` for a managed run. Inspect the reported run directory or
`just agent-debug last` instead.

## Leave the managed path when necessary

Do not contort an unsupported experiment into this runner. If the task genuinely needs LLDB, an
unmanaged protocol conversation, custom Cargo build configuration, a different profiler, or
another command, state why the managed workflow does not apply and request the necessary approval.
