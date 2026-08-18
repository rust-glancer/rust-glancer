# Limitations

Rust Glancer is incomplete, and has a bunch of quirks that are worth knowing about.

## Ultimate advice

If something unexpected happens: you start getting a lot of errors, index is messed up, LSP stops responding, etc:
- Try hitting ctrl/cmd+shift+P and sending `Rust Glancer: Reindex workspace` command.
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
