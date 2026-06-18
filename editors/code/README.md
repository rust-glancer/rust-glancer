# Rust Glancer VS Code Extension

VS Code client for `rust-glancer lsp`.

## Development

Install dependencies and build the bundled extension:

```text
npm install
npm run compile
```

Launch `Run Rust Glancer Extension` from VS Code. The launch configuration
opens the repository root in an Extension Development Host.

During development the extension starts the configured `rust-glancer`
executable, or `rust-glancer` from `PATH` when no path is configured. Build the
server binary first if needed:

```text
cargo build --release -p rust-glancer
```

Then point the extension at that binary:

```json
{
  "rust-glancer.server.path": "/absolute/path/to/rust-glancer/target/release/rust-glancer"
}
```

## Testing

Run the unit tests:

```text
npm run test:unit
```

Run the extension-host smoke test:

```text
npm run test:e2e
```

The VS Code test runner uses a desktop Extension Development Host, so a short
lived VS Code window is expected locally. For Linux CI/headless environments,
run the same command under a virtual display such as `xvfb-run`.

For faster CI checks that do not launch VS Code:

```text
npm run fmt:check
npm run lint
npm run check
npm run check:test
npm run check:unit
```

The same checks are available through the client Justfile:

```text
just lint
# From the repository root:
just client lint
```

## Useful Settings

```json
{
  "rust-glancer.server.path": null,
  "rust-glancer.server.extraEnv": {},
  "rust-glancer.cargo.target": null,
  "rust-glancer.cfg.test": true,
  "rust-glancer.cfg.atoms": [],
  "rust-glancer.cache.packageResidency": "workspace-and-path-deps",
  "rust-glancer.trace.server": "off",
  "rust-glancer.diagnostics.onStartup": false,
  "rust-glancer.diagnostics.onSave": false,
  "rust-glancer.diagnostics.command": "check",
  "rust-glancer.diagnostics.cargoArguments": ["--workspace"],
  "rust-glancer.diagnostics.extraEnv": {}
}
```

Use `rust-glancer.server.extraEnv` for server logs, for example:

```json
{
  "rust-glancer.server.extraEnv": {
    "RUST_GLANCER_LOG": "rg_lsp_server=debug,rg_lsp_engine=debug"
  }
}
```

Use `rust-glancer.diagnostics.extraEnv` for environment variables that should
only affect Cargo diagnostics, for example custom cfg flags:

```json
{
  "rust-glancer.diagnostics.extraEnv": {
    "RUSTFLAGS": "--cfg tokio_unstable"
  }
}
```

## Troubleshooting

Open the `Rust Glancer` output channel first. It records the workspace root,
server command, server source, process exit, server stderr, and engine startup
logs.

If the command palette does not show Rust Glancer commands in the Extension
Development Host, VS Code probably launched with the wrong
`extensionDevelopmentPath`; use the checked-in launch configuration.
