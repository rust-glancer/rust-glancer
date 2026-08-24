# Limitations

Rust Glancer is incomplete, and has a bunch of quirks that are worth knowing about.

## Ultimate advice

If something unexpected happens: you start getting a lot of errors, index is messed up, LSP stops responding, etc:
- Try hitting `ctrl/cmd+shift+P` and sending `Rust Glancer: Reindex workspace` command.
- Try restarting the server: click on `Rust Glancer` on the bottom left of VS Code.
- If that doesn't help, stop the editor, remove `target/rust_glancer`, and start again.

If you know how to reproduce the issue, it would be great if you also [report it](https://github.com/rust-glancer/rust-glancer/issues).

It shouldn't happen often, but you know how it is with young software.
I intentionally don't implement any sophisticated recovery mechanisms: the idea is that the LSP should never fail,
so when it does, it has to be loud. So if you meet a crash or indexing issue -- sorry, but I hope that it will
help us build a very reliable project long term.

## Dirty buffers

Frozen workspace analysis can work on each keystroke, and it is actually usable, but it falls
into a category that is workable but annoying enough to drive one insane. So to mitigate that,
dirty buffers use partial analysis: we recompute the bodies that were affected, and
we perform a somewhat "shallow" analysis.

It means that we can infer types as you type inside of the function, the completions for
already known structures/functions/traits will work, but we cannot see new items.
If you type the following without saving:

```rust
struct FooBar;

impl Foo$
```

you will not see `FooBar` in completions, because adding structure to analysis will require doing
a lot of extra work. Similarly, we cannot add imports to the scope as you type, so the `HashMap`
will not be suggested if you will type the following without saving:

```rust
use std::collections::HashMap;

fn foo(h: HashMa$)
```

So it's important to obtain a mental model where you need to save whenever you add something meaningful
-- struct, trait, import, function.

It might take a bit of time to adjust, but if your flow wasn't like this already, I can promise that it
might feel pretty natural after a bit of time.

## Build scripts

Rust Glancer has an intentional policy where aims not to execute code. Part of it is performance-related,
part of it is security-related. This is also the reason why diagnostics are disabled by default -- the
project philosophy is that enabling diagnostics must be a user's decision, not something shipped by default.

Because of that, we don't proactively execute build scripts for projects.
Instead, we try to observe the existing artifacts and use them.
This stage is optional, so failure to discover cargo build artifacts does not stop or fail indexing.

So the logic is roughly:

- if the code has never been built or `cargo clean` has been run, the analysis
will not contain any build script outputs, and most likely it will result in some imports not being resolved.
The analysis will still finish and be available to the possilbe extent.
- if the code has been built a while ago, outdated artifacts may be used for analysis. In many cases it's fine,
since build script outputs do not change that often.
- if newest possible build script outputs should be used for indexing, a manual reindexing command can be sent
to the server after running `cargo check` / `cargo build` (e.g. through `ctrl/cmd+shift+P` in VS Code and
"Rust Glancer: Reindex Workspace" chosen there). This way user will first make sure that cargo build artifacts
are updated, and then will prompt the LSP to observe them.

Note that if newer artifacts appear during normal reindexing flow, they will NOT necessarily be utilized.
Standard reindex-on-save does not check if there is a newer cargo build output. The reason for that is because
in most cases reusing past outputs is acceptable, and eager reindexing could make user experience significantly
worse.

This is a somewhat manual workflow where user is responsible for providing the outputs that need to be used
by Rust Glancer. This model is a compromose to make build scripts supported, while not making LSP responsible
for building the actual project to work.
