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

During development the extension starts the server through Cargo:

```text
cargo run --release -p rust-glancer -- lsp
```

To force a specific server binary, set:

```json
{
  "rust-glancer.server.path": "/path/to/rust-glancer"
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

This builds the real `rust-glancer` release binary, opens
`test_targets/simple_crate`, activates the extension, waits for the server to
be ready, and runs the reindex command.

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
  "rust-glancer.cache.packageResidency": "workspace-and-path-deps",
  "rust-glancer.trace.server": "off",
  "rust-glancer.checkOnStartup": false,
  "rust-glancer.checkOnSave": false,
  "rust-glancer.check.command": "check",
  "rust-glancer.check.arguments": ["--workspace", "--all-targets"]
}
```

Use `rust-glancer.server.extraEnv` for server logs, for example:

```json
{
  "rust-glancer.server.extraEnv": {
    "RUST_GLANCER_LOG": "rg_lsp=debug"
  }
}
```

## Troubleshooting

Open the `Rust Glancer` output channel first. It records the workspace root,
server command, server source, process exit, and server stderr.

If the command palette does not show Rust Glancer commands in the Extension
Development Host, VS Code probably launched with the wrong
`extensionDevelopmentPath`; use the checked-in launch configuration.
