# Rust Glancer for Zed

This extension connects Zed's built-in Rust language support to
`rust-glancer lsp`. The proof of concept expects the Rust Glancer executable to
be installed on `PATH` or configured explicitly; it does not download a server
binary.

## Development

Build the language server first:

```text
cargo build --release -p rust-glancer
```

In Zed, run `zed: extensions`, choose **Install Dev Extension**, and select the
`editors/zed` directory from this repository.

For a deterministic local setup, add the following project settings and replace
the binary path with the path to this checkout:

```json
{
  "languages": {
    "Rust": {
      "language_servers": ["rust-glancer", "!rust-analyzer"]
    }
  },
  "lsp": {
    "rust-glancer": {
      "binary": {
        "path": "/absolute/path/to/rust-glancer/target/release/rust-glancer",
        "arguments": ["lsp"]
      }
    }
  }
}
```

The extension uses `["lsp"]` when `binary.arguments` is omitted. If arguments
are configured, they replace that default and must retain the `lsp` subcommand.
When `binary.path` is omitted, the extension looks for `rust-glancer` on the
project's `PATH`.

Rust Glancer applies its server-side configuration defaults when no
initialization options are provided. Overrides use Zed's standard
`initialization_options` object:

```json
{
  "lsp": {
    "rust-glancer": {
      "initialization_options": {
        "cfg": {
          "test": true,
          "atoms": []
        },
        "diagnostics": {
          "onSave": true,
          "command": "check",
          "cargoArguments": ["--workspace"]
        }
      }
    }
  }
}
```

See the project configuration guide at
[`docs/src/usage/CONFIGURE.md`](../../docs/src/usage/CONFIGURE.md) for the full
set of initialization options.

To enable server logs, set an environment override on the language-server
binary:

```json
{
  "lsp": {
    "rust-glancer": {
      "binary": {
        "env": {
          "RUST_GLANCER_LOG": "rg_lsp_server=debug,rg_lsp_engine=debug"
        }
      }
    }
  }
}
```

Use Zed's `dev: open language server logs`, `editor: restart language server`,
and `editor: stop language server` actions while troubleshooting.

## Building the extension

Zed compiles development extensions automatically. To check the WebAssembly
build directly:

```text
rustup target add wasm32-wasip2
cargo build -p rust-glancer-zed --target wasm32-wasip2
```

The extension version in `extension.toml` and publication to the Zed extension
registry are intentionally updated manually for now.
