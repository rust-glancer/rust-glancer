# Memory approach

This project cares a lot about memory allocations. This document outlines
how this is done, and what tooling and techniques are commonly used for
that purpose.

## Offloading & purging

The two biggest memory savers in this project are offloading and memory purging.

Offloading lets us write computed analysis data to disk and only load
the relevant bits for the duration of the query.

Purging is a `jemalloc` feature that lets us force the allocator to return
memory to the system. Typically, even if some allocations were freed, the allocator
will not rush to return memory to the system, since this memory may be reused for
new allocations. In our case, however, we care about idle RAM much more than
about peak performance: LSP is human-driven, so the queries appear on the "seconds"
timeline, not "microseconds", and we don't really worry about a few more syscalls.

That said, while these techniques reduce idle memory usage, they are not a replacement
for optimizing the phase itself. Purging can reduce RSS across phases, but it does
not reduce peak RSS within a phase per se.

One of the caveats, for example, is memory fragmentation: if memory is
significantly fragmented, purging will not help much, since the allocator will not
be able to give big enough chunks back to the OS.

So, the rest of the document is focused on the particular memory optimization
techniques we use _outside_ of purging and offloading.

### Startup cache loading

Honorary mention: we don't only offload cache while LSP is active; since the cache
is already on the filesystem, we try to reuse it on relaunch. Cache is fingerprinted, so
on startup we're evaluating whether the cache is compatible with the current state of
the project, and if so, we can get indexed workspace at startup for free.

This is intentionally opportunistic and per-package: the indexing remains reasonably
fast anyway, so if package cache probing fails for any reason, we treat it as a miss and
re-index the corresponding package from source.

## Memory optimizations

### Jemalloc stats & notation

The core command for analyzing memory allocations is `just analyze`, and it prints
plenty of stats. The "default fixture" for that is `rust-analyzer` on a specific
commit, since it's a big enough, stable, and complex "real-world" project. It can
be fetched by running [`test_targets/bench_fixtures/fetch-rust-analyzer.sh`][ra].

One key part of the output is the `project.build.checkpoints` profile table:

```bash
$ just analyze path/to/rust-analyzer --profile --package-residency all-offloadable -m
# .. some parts omitted
profile snapshot:
 phase     elapsed    rg_sampled      rg_total   j_allocated      j_active    j_resident      j_mapped  checkpoint
150 ms      150 ms      60.8 MiB      60.8 MiB      70.6 MiB      76.9 MiB      83.3 MiB      85.7 MiB  after parse
  0 ms      151 ms       2.3 KiB      60.8 MiB      70.6 MiB      76.9 MiB      83.3 MiB      85.7 MiB  after cache probe
1.21 s      1.36 s     227.2 MiB     285.0 MiB     282.7 MiB     500.7 MiB     602.9 MiB     628.0 MiB  after item-tree
 58 ms      1.42 s      26.0 MiB     274.8 MiB     273.0 MiB     493.8 MiB     504.8 MiB     529.8 MiB  after item-tree syntax eviction
157 ms      1.58 s      16.5 KiB     274.8 MiB     273.3 MiB     494.1 MiB     512.6 MiB     537.6 MiB  after cache source fingerprints
3.80 s      5.38 s     126.8 MiB     402.4 MiB     396.9 MiB     641.8 MiB     660.4 MiB     683.8 MiB  after def-map
253 ms      5.63 s     159.7 MiB     562.1 MiB     543.5 MiB     783.2 MiB     831.3 MiB     854.7 MiB  after semantic-ir
145 ms      5.77 s             -     332.4 MiB     331.1 MiB     421.6 MiB     831.3 MiB     854.7 MiB  after item-tree drop
1.92 s      7.70 s     278.1 MiB     613.4 MiB     610.0 MiB     691.7 MiB     711.0 MiB     733.7 MiB  after body-ir
140 ms      7.84 s      26.0 MiB     600.5 MiB     593.1 MiB     663.1 MiB     714.5 MiB     737.2 MiB  after parse syntax eviction
 91 ms      7.93 s     603.7 MiB     603.7 MiB     591.8 MiB     660.8 MiB     714.5 MiB     737.2 MiB  before package cache write
1.34 s      9.27 s     603.7 MiB     603.7 MiB     599.1 MiB     675.0 MiB     911.6 MiB    1001.6 MiB  after package cache write
214 ms      9.48 s      36.0 MiB      36.0 MiB      46.2 MiB     110.5 MiB     675.8 MiB     698.2 MiB  after package payload offload
 17 ms      9.50 s       7.3 MiB       7.3 MiB      10.6 MiB      38.2 MiB     675.8 MiB     698.2 MiB  after package offload cleanup
 23 ms      9.52 s       7.3 MiB       7.3 MiB       7.8 MiB      33.3 MiB      53.0 MiB      75.3 MiB  after project
```

The information should be interpreted as follows:

- `checkpoint` denotes the "logical checkpoint"
- `phase` is the duration of this step _only_
- `elapsed` is the duration since the start of the indexing
- `rg_sampled` is memory measured for the checkpoint's main data structure
- `rg_total` is memory measured for all known live build state
- `j_allocated` is the jemalloc stat for memory the program is actually using
- `j_active` is the jemalloc stat for internal allocated pages that serve `j_allocated`.
    It is normal for it to be somewhat higher, since memory is allocated in pages, and
    `j_allocated` typically does not fully utilize each page.
- `j_resident` is the jemalloc stat that includes dirty pages and other jemalloc-internal state.
- `j_mapped` is the mapped OS range, not necessarily fully allocated.

Note: `rg_total` is an _estimate_, since we can't count everything perfectly, but it should be
_close enough_ to `j_allocated`. The reason why it's important is because our internal
harness also records _what_ is allocated and _where_: as long as it's accurate, we can make
educated guesses on where we can save on allocations themselves.

Here are some heuristics that can be used to reason about this table:

1. `j_allocated << j_active ~= j_resident` -- likely active memory fragmentation issue.
  Rough explanation: there are few allocations, but they take a lot of allocated pages.
2. `j_allocated < j_active << j_resident` -- purging might help.
  Rough explanation: we have experimented with different jemalloc configurations, but the default
  configuration appears to be a good fit for this project, so in most cases explicit purging is the
  lever we care about.
3. `j_resident << j_mapped` -- not necessarily bad, but might be worth investigation, might
  signal excessive transient allocations in the past and _could_ signal unexpected peak RSS.

Important caveat: this table prints data _after the phase_, so it does not represent _peak_
allocation, which can be much higher during the phase because of transient allocations.
For peak RSS, extra profiling might be required.

[ra]: ../test_targets/bench_fixtures/fetch-rust-analyzer.sh

### Layered allocations

The core memory optimization we have is layered allocations. A typical expected
flow for lowering would be per-package, e.g. do "item tree -> defmap -> semantic -> body"
for each package. This is expected, but it has a flaw: interleaving allocation lifetimes.

Some of the data during lowering/resolution is transient, and it will be removed after the
phase. If we do all the phases for each package separately, it will leave a lot of
"holes" in memory, which will be filled by allocations from the newly processed packages,
which... Well, in the end we will end up with highly fragmented memory.

What we do instead is optimize for _memory allocation lifetime_. We lower the same phase
for all the packages first, and then we shrink and compact it.

During lowering, the most important bit to think about is whether the allocations from
this phase will intersect with allocations from the next phase: it's bad if they will.

An example: during item tree lowering, we should not do parsing in parallel, since two
phases have different lifetimes. We should _first_ parse files, and _then_ perform item
tree lowering, since item tree lives longer than syntax, and parse syntax eviction should
free up a continuous chunk of memory, not slots between item tree allocations.

### Compacting

Compacting is important: imagine that we have a `Vec` of interim `Builder` objects and
we need to "freeze" it into a `Vec` of "built"/"frozen" objects. If we shrink
the object and then allocate the "final" object right away, it will likely go to
the just-freed memory space. And if we repeat it for every object, we will end up
with a lot of holes between objects.

Instead, we can shrink all the objects, and _while the old objects are alive_ allocate
a vector of new objects: these will be compact in memory, since we will allocate the
whole chunk right away with no gaps. After that, it will be safe to drop the "old"
objects -- they will free up all the capacity. Given that this will happen at the end
of the stage and will be followed by a purge, we'll end up with cleaner memory layout.

Note: bump allocators _could_ help with transient storage, but they have their own costs;
we've briefly experimented with these, and the result was that they yield similar results
to manually optimized allocations, but have much worse ergonomics, and in some cases can
worsen peak RSS due to arena allocation overhead.

### Eager eviction

It was already mentioned above, but eager eviction is also one of the major strategies:
we keep data only while it's needed. For example, parse syntax _globally_ is only needed
until we lower item tree. After that, we can lower semantic tree based on item tree only.
For body layer, we will need syntax again, but on a per-file basis: we parse it for the
file only, lower all the bodies, and then drop it.

### Parallelism tweaking

We use parallelism heavily for indexing, but while a higher thread count can make
things faster, it can also increase the volume of transient allocations: more threads
will consume more pages, while not keeping them fully utilized. This will be freed
after the phase, of course, but it can increase the peak RSS quite heavily.

The main current offender is declarative macro expansion: it allocates a lot of transient
syntax trees, and on my current machine unconstrained macro expansion uses 16 threads and
processes the defmap stage in ~3.2s with ~3.1GB peak RSS, while capping macro expansion to
2 threads increases defmap time to ~3.9s, but lowers peak RSS to ~1.8GB.

Here it's a trade-off, so we let users choose what they value more: peak
RSS or indexing speed.

## Memory tooling & techniques

This section outlines the strategies you can use to analyze memory allocation
patterns during development.

### `MemorySize` & `Shrink`

These traits and derive macros are core workspace infrastructure for memory measurement
and compaction.

- `MemorySize` allows recording the size of the object, including not only the shallow
  size of the object itself, but also the size of its children and approximate overhead.
  It can distinguish certain and approximate allocations, and it can record what is being
  allocated and where (e.g. object type and object scope). These measurements are not
  automatic, but derive support makes them convenient to add in all the necessary
  places. It is possible to forget it somewhere, but so far it's pretty consistent, and
  through profiling it is possible to detect if counting becomes very inaccurate.
- `Shrink` allows recursively shrinking allocations, where possible. It is mainly relevant
  for stored `std`-backed containers with a `shrink_to_fit` method. During indexing, quite
  a lot of containers might have unpredictable size and end up with spare allocated capacity.
  Based on experiments, so far it has proven to be a better solution ergonomically than bump
  allocators.

`MemorySize` and `Shrink` usually go hand in hand -- if you want to measure something, you
probably also want to shrink it, and if you want to shrink something, it is certainly worth
measuring.

### `just analyze`

This is the baseline command. For details, run `just analyze --help`.

Useful flags:

- `--profile [selectors]` collects dynamic profile data. Without a value, it uses
  the `default` alias, which records build checkpoints. `--profile all` collects
  every registered profile.
- `-m` / `--memory` adds retained-memory and jemalloc stats to build checkpoints,
  plus the final retained-memory breakdown.
- `--profile memory:def-map` records a detailed memory breakdown for the def-map
  checkpoint. Other available aliases and selectors are listed in `just analyze --help`.
- `--package-residency <policy>` controls what remains resident after indexing.
  `all-offloadable` is useful for checking idle-memory behavior, while `all-resident`
  is useful when you want to remove offloading from the experiment.
- `-l` / `--load` enables startup cache loading, so matching offloadable package
  artifacts can be reused instead of rebuilt.
- `--indexing-preference <preference>` switches between `lower-peak-memory` and
  `faster-builds`.
- `--profile macros` prints defmap macro-expansion counters, timings, and by-name tables.
- `--format json` is useful if you want to compare reports with a script.

Note: this command works for _both_ performance and memory analysis, but memory
analysis is not free -- it makes indexing slower. So for accurate data on performance,
use this command without `-m`/`--memory`.

Samples:

"Default go-to command" -- analyze with full offloading
```
just analyze path/to/rust-analyzer --profile --package-residency all-offloadable -m
```

Exclude offloading from analysis:
```
just analyze path/to/rust-analyzer --profile --package-residency all-resident -m
```

Check what a real startup with cache hits looks like:
```
just analyze path/to/rust-analyzer --profile --package-residency all-offloadable --load -m
```

Inspect one suspicious checkpoint in more detail:
```
just analyze path/to/rust-analyzer --profile memory:def-map --package-residency all-offloadable -m
```

Compare the speed/memory trade-off for macro expansion:
```
just analyze path/to/rust-analyzer --profile --package-residency all-offloadable --indexing-preference faster-builds -m
just analyze path/to/rust-analyzer --profile --package-residency all-offloadable --indexing-preference lower-peak-memory -m
```

If defmap is suspicious, add macro stats:
```
just analyze path/to/rust-analyzer --profile default,macros --package-residency all-offloadable -m
```

For disabling memory purging, you can set `RUST_GLANCER_PURGE_MEMORY_AFTER_BUILD=0`
environment variable.

### Peak RSS

The above commands will _not_ give you peak RSS. For peak RSS, a good idea is to use
`time`.

For `zsh`:

```
TIMEFMT='%J  %U user %S system %P cpu %*E total %M max_rss'
time just analyze path/to/rust-analyzer --profile --package-residency all-offloadable -m
```

Note that depending on platform, `max_rss` might be in KiB or MiB.
