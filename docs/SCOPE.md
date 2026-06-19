# Project scope

LSP is an ambitious project, and I have to choose the scope I can realistically
accomplish.

At least, at the time of writing, my goals are (in that order):

- Extreme memory efficiency, <100mb RSS per active engine for any realistic project.
- Fast indexing and instant startup with an existing index. Launching an IDE after you
  booted your PC shouldn't make you wait a minute or two with CPU going brrr.
- Maintainable. I am a grug brain, so I don't like overly smart code. It is important
  that I know the whole codebase and can orient myself there.
- Provide enough data for day-to-day work. Normal things should work kind of well;
  I'm fine with some things not being implemented as long as I get ~85% of a complete
  LSP.

As a result, the following things are expected:

- Type inference is basic and best-effort.
- Trait solving is basic and best-effort.
- No proc macro support.
- No build script resolving.

An additional implication is:

**No unneeded features**. If something is added, it means that it affects a significant
chunk of users. "Good to have because why not" features are not really for this project.

An example of that is unstable nightly features, especially ones that are expected to
change often. _At least_ until the project provides good enough coverage for stable,
we don't want to start working on nightly-only features (_especially_ big ones like const
generics or specialization).

Some of these things _might_ change in the future, but only if they don't sacrifice the
goals stated above. I can see, for example, basic autoderef implemented in the future,
but it is extremely unlikely that I'll ever integrate a real trait solver. It is even
more unlikely that this LSP will become incremental.

The reason for that is that I believe that Rust developers are usually not that stupid
and can compensate for the LSP incompleteness, grep if required, or, if the code is
so complex that it cannot be understood without an LSP, simplify it.

For people who, for any reason, need to rely on a complete LSP, there is a _really_ great
one -- rust-analyzer. It's super cool, it is build by people who are much smarter than me,
and I don't want to compete with it. Each project can have its own niche.
