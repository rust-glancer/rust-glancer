# Configuration

Rust Glancer comes with a configuration that is meant to be optimal for casual use:

- All packages are offloaded to filesystem (minimal RAM usage).
- Bodies are only indexed for the workspace (dependencies receive semantic analysis only).
- Some tweaks that can change burst RAM/speed balance during indexing are optimized for indexing speed.

There is one caveat: `cargo check` (or rather, any diagnostics) are disabled by default. It is intentional:
since the project is all about being non-intrusive, I think it should be a conscious choice to enable
diagnostics. So if you need them, please go to settings and enable diagnostics on startup/save.

Otherwise, typically you don't need to tweak the settings to make something work better. You can, though.
Tweaking options are covered from the more "traditional" configuration options.

## Configuration options

You can configure all the typical things you might want to configure:

- Command for diagnostics and arguments for it (`cargo check` by default).
- Enable/disable diagnostics on startup / on save. Keeping them disabled makes Rust Glancer indexing flow feel significantly faster, so it can be reasonable if you use something else to observe diagnostics, e.g. [bacon](https://github.com/canop/bacon).
- Configure cargo features / enable all features / disable default features.
- Configure cargo target triple, cfg atoms (e.g. custom `cfg` attributes), and `cfg(test)`.
- Extra env vars for cargo commands (e.g. `RUSTFLAGS`)

What's important here is that there is a (somewhat poorly named) `Cargo: Overrides` config.
It allows you to override cargo target and feature settings _per cargo workspace_. This is useful
if one VS Code workspace has several cargo workspaces inside. Imagine that one project compiles to
RISC-V only, another is only for linux, and they have conflicting features: you can still keep them
open in one VS Code instance at the same time using this feature.

The field is an array of objects. Each object has a `path` for the exact cargo workspace root,
absolute or relative to a VS Code workspace folder. It can override `target`, `allFeatures`,
`noDefaultFeatures`, and `features`.

For example:

```json
{
  "rust-glancer.cargo.overrides": [
    {
      "path": "firmware",
      "target": "riscv32imac-unknown-none-elf",
      "noDefaultFeatures": true,
      "features": ["board-v1"]
    }
  ]
}
```

Another (maybe) important configuration is `Indexing: Performance Preference`:
`lower-peak-memory` makes the indexer use less concurrency, making it slower, but also reducing peak
allocations during indexing. Exact reduction can vary by machine/os/project, so if the default
(faster builds) does not work for you and you are affected by high peak RAM, you can try changing
this option.

## Tweaking options

Following settings _can_ be changed, but probably shouldn't.

- `Cache: Package Residency`: you can make it so that not everything is offloaded to the filesystem.
  In theory, it can make Rust Glancer faster. In practice, if you don't care about memory usage,
  `rust-analyzer` will probably work better for you.
- `Server: Purge Memory After Build`: probably shouldn't even be a configuration option. Returns
  unused memory back to OS with no visible downsides.
