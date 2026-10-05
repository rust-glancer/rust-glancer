# Installation

Before anything else, `rust-src` is mandatory for the project to work correctly,
so don't forget to run `rustup component add rust-src` if you're not sure if you
have it installed.

When you install Rust Glancer, do not forget to turn rust-analyzer off. They should work
together just fine (I've done that and did not have any conflicts), but it's kind of
meaningless to run both.

## VS Code

You have two options:
1. [Install the extension from the official marketplace](https://marketplace.visualstudio.com/items?itemName=rust-glancer.rust-glancer).
2. Build and install VSIX from the repository.

The extension is maintained and will be updated, but given that VS Code extensions
often become targets of attacks nowadays, I'd probably recommend building from source
(or at least disabling auto-updates). Please do not forget to update it from time
to time though. There will be good things in updates (probably).

### Forks (cursor, vscodium, etc)

[Install the extension from OpenVSX](https://open-vsx.org/extension/rust-glancer/rust-glancer)

### Installing from VSIX

0. (Optional) Install [just](https://github.com/casey/just)
1. Clone the [repository](https://github.com/rust-glancer/rust-glancer)
2. Run `just package-vsix` (or go to `editors/code` and build via `npm`)
3. Open VS Code, navigate to extensions tab, click on `...` and choose `Install from VSIX`.
4. Install the extension.
5. ???
6. PROFIT

## Zed

Install the `Rust Glancer` extension from the marketplace.

Minimal required configuration in settings (you need to disable rust-analyzer, which is enabled by default, and enable Rust Glancer):

```json
  "languages": {
    "Rust": {
      "language_servers": ["rust-glancer", "!rust-analyzer"],
    },
  }
```

## nvim

There exists a [nvim-lspconfig configuration](https://github.com/neovim/nvim-lspconfig/blob/3e8d598d3b5f8338a41699c436e5fa11d2666cf0/lsp/rust_glancer.lua) contributed by
[@h-michael](https://github.com/h-michael).

It's also present in [mason registry](https://github.com/mason-org/mason-registry/blob/main/packages/rust-glancer/package.yaml), contributed by [@macroexpansion](https://github.com/macroexpansion).

Currently I'm not maintaining these, so the pinned version might be outdated. I don't plan any client changes anytime soon, however, so feel free to grab the prebuilt version from the link below and configure Rust Glancer to use it.

## Installing the server

- You can fetch prebuilt binaries on the [releases page](https://github.com/rust-glancer/rust-glancer/releases).
- For windows, there is [Scoop support](https://github.com/ScoopInstaller/Main/pull/8628) (at the time of writing, 0.3.0).

## Other editors

Any editor that supports LSP protocol, should work with Rust Glancer.

Typically all you need is to configure Rust Glancer binary to be the Rust LSP, no special configuration
should be required. If you'll notice any quirks, please [create an issue](https://github.com/rust-glancer/rust-glancer/issues) in the repository.

Also, contributions with configuration guides for other editors are welcome (but please make sure that they work; do not submit untested LLM-generated PRs).
