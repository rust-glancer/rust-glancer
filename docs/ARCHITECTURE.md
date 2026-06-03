# Architecture

This project implements an LSP server and VS Code LSP client extension.
The server supports multiple workspace folders per project, and at a high level
it works as follows:

- The client talks to a single LSP server implementation.
- The LSP server implementation acts as an orchestrator over different engines,
  one engine per "real" workspace.
- An engine owns the implementation of LSP "functionality", e.g. indexing a workspace
  and answering queries.

Whenever a Rust document is opened, the extension activates and asks the LSP server to
initialize. Upon a document opening, the LSP server resolves the workspace root for it,
checks that there is no active engine for this workspace, and starts a new engine.

Then, when a new document is opened in the same LSP project, it will either be routed
to an existing engine, or, if the document doesn't belong to any open workspace, a
new engine will be spawned.

For dependencies, we try not to spawn an engine, since engine analysis already covers
them. Right now, we don't have any fancy logic there, and we assume that Rust files
outside of the project folder should be handled by the last active engine; e.g.
the expectation is that _typically_ one gets outside of the project folder in the editor
when they're following `goto definition` or some similar query.

Each engine is spawned as a subprocess, and communication between engines and the LSP server
is implemented using `tarpc`.
The communication is quasi-bi-directional: each engine exposes a `tarpc` service that
the LSP server can interact with; the service is not LSP, it's a domain-specific implementation,
though obviously heavily LSP-shaped.
Additionally, the LSP server itself exposes a service that all the engines can use to send
_notifications_, e.g. diagnostics or progress reports.
So, the LSP server can send _queries_ and _notifications_ to engines, while engines can only
send _notifications_ to the LSP server.

## Engine

The engine is built to be used by the LSP server, but it is not tied to one; the implementation
is protocol-agnostic, and there exists an integration layer that converts the engine
data to LSP data.

The engine normally starts by indexing the workspace. Indexing has the following phases:
- Metadata inspection: call `cargo metadata` and extract dependency graph and other
  useful things.
- Parsing: going through every crate and converting raw text to AST.
- Item tree building: converting raw AST nodes to the basics of our internal representation,
  without any attempt to analyze them yet.
- DefMap building: resolving modules, imports, and item "locations". At this stage, we know what
  exists where. Additionally, at this stage we resolve declarative macros.
- Semantic IR building: resolving information on the _item_ level, e.g. struct declarations, traits,
  which structures implement which traits, etc. This includes the macro-generated items too.
- Body IR building: resolving information on the _body_ level, e.g. within a function body, which
  variable has which type. Body data is the heaviest across everything else, both in terms of size
  and complexity, so by default we only analyze bodies in the workspace, assuming that you don't
  need information about bodies in your dependencies. It is configurable, though.

All of this information is collected into a `Project`, which represents a frozen snapshot of
collected data.

Additionally, for a `Project`, we can create an `Analysis`, which can be used for queries, e.g.
to ask which traits this type implements or what completions are available at this cursor.

To support `Analysis`, we have a bunch of helper crates that provide abstract interfaces and
query surface, such as `ir-storage` / `ty` / `ir-view`.

### Frozen workspaces

Probably the most important fact about the engine is that it is _frozen_, not _incremental_.
This means that we index the workspace once, and then run the queries against the data we've
managed to collect. If something changes, we have to reindex (at least partially); we cannot
update the data in place.

Dirty buffers are supported as overlays that retain access to existing analysis but does not
automatically index new facts. So things like completions, hover, inlay hints work, but with
limitations. Reindexing happens on save.

It is a limitation, but  working with frozen workspaces enables really cool memory optimizations
that we make heavy use of. These are described below.

## Memory efficiency

Memory efficiency is _the_ main goal of this project. A lot of optimizations are implemented for
that.

### Allocation optimizations

Wherever possible, we use arenas for allocations, and we try to only keep things in memory
that are really needed. So, for example, during indexing we drop `ItemTreeDb` once we've
collected all the information we needed from it.

The approach to memory allocations is twofold:
- If you don't need to allocate, do not allocate.
- If you allocate, try to do it in a way that minimizes fragmentation.

### Jemalloc

We use jemalloc, since it reduces memory fragmentation, exposes memory statistics, and
gives control over the allocator.

Using jemalloc-ctl, we try to purge memory each time we assume that we've done
a bunch of allocations that are no longer used. It's fast enough for users to not notice
during normal LSP interaction flow, and it keeps RSS as low as possible.

Because of that approach, memory fragmentation can be a _huge_ issue: if we allocate
a bunch of things in random locations, the `allocated` memory can stay low, but `active`
will be high, meaning that we cannot return memory to the OS.

### Package offloading

This is the biggest win. Since we're using frozen queries, the index can be offloaded to
the file system. And the engine can work on _top_ of an offloaded index, e.g. the index will
only be loaded in a transaction for the relevant packages, and only for the duration of
the query. Then, the transaction will be dropped, and memory is freed.

As a cool side effect, the engine starts immediately if the index is not invalidated, so
you get instant or almost instant restarts.

The offloading is per-package, so invalidation of a single package does not mean
invalidation of the whole index. The intended approach is to offload everything, but
it's possible to choose what you actually want to offload.

### Engines in separate processes

Separate processes are not technically required for the engines, but if we had multiple
engines in the same process, we could get into scenarios where a new engine is spawned while
another engine is indexing, meaning that allocations will happen in random places, and
the memory will be extremely fragmented.

Keeping each engine in a separate process makes it much easier to ensure that the memory
layout is stable.

## Code layout

- `crates/rust-glancer` defines the binary
- `crates/lsp` defines the components of the LSP server, e.g. the server implementation
  itself, engine LSP adapter, and shared protocol.
- `crates/engine` defines the engine components.
- `crates/lib` defines the shared semi-general-purpose libraries.
- `editors/code` defines the VS Code extension.
