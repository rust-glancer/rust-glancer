# Rust Glancer

Rust Glancer is an experimental LSP implementation that tries to use a different architecture
compared to [rust-analyzer](https://rust-analyzer.github.io/) in order to lower RAM usage
and make editor restarts faster.

rust-analyzer keeps all the data in RAM and uses incremental model that lazily recomputes
data that is needed to provide editor support.

Rust Glancer instead uses frozen workspaces: indexing is performed eagerly, and thanks to
that we have all the data we need, so we can store it on the filesystem. After indexing is
done, data is stored on the filesystem, and is only loaded for the duration of the LSP query.

It means that:
1. Idle RAM usage is extremely low, <100mb even for big projects.
2. Since we store data on filesystem, if the project was indexed and you relaunch the editor
  without changing the code, you get your indexed workspace nearly instantly.

It does not come for free, though:
- Query execution on average is slower than in rust-analyzer, though it is still in an "acceptable"
  territory from the human perception (e.g. think that completions show up in 100s of milliseconds,
  not in 10s of seconds).
- Since we eagerly index everything, indexing _might_ be slower than one in rust-analyzer (though it
  depends on project and configuration).
- In order to make editing acceptable, we need to use some hacks, making analysis of dirty buffers
  less precise than one of saved buffer. Indexing happens only on save, so when you're typing we're
  performing a "shallow" analysis. It's better covered in [limitations](../usage/LIMITATIONS.md).

And obviously Rust Glancer is a much younger project than rust-analyzer, so it's less complete:
- Type inference doesn't work in some cases
- There are bugs here and there
- Advanced features like proc macros are not supported.

For some the above can be a deal breaker, but I'm using Rust Glancer to build Rust Glancer (and
for other projects I'm working on) and I find it sufficient even in its current form. And oh boy
do I enjoy this <100mb RAM usage.

So if that doesn't scare you, you are very welcome to [install](../usage/INSTALL.md) and try it out.
Hope you'll like it!
