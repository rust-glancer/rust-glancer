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
dirty buffers cheat a bit: when hover/completion/etc is requested in a dirty buffer, we take
the _current entity around the cursor_ and analyze its current state against the saved project.

This is already enough for typing, and in this example (`$` denotes the cursor) you will see
the completions:

```rust
fn calculate_something() {
    let foo = SomeComplicatedType::new();
    foo.do_some$
}
```

Local variables, types, methods are also available.

However, it gets tricky with new impl blocks. We try to recover new items based on syntax,
but we don't add them to the project-wise analysis, so, for example, you will see methods from a
newly typed `impl` block as you type them, same for the trait impls, but they will only work while
you are inside of the corresponding block. If you will move to a different `impl`, it will not see
the changes from the `impl` you edited before until you save.

Module-level changes, such as adding a new import, are also not automatically resolved, they only
take effect after saving a file.

So for almost all the normal flows it should be convenient, but you will need to save the file when
you need a change you made to take effect outside of the place where you type. 

If it sounds scary, just try it -- it really isn't,
and you can get used to it pretty quickly. And thanks to that, editing experience remains smooth.

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
