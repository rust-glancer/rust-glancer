# Project scope

LSP is an ambitious project, so it can only exist if the scope is bounded.

At least, at the time of writing, the goals of the project are (in that order):

- Extreme memory efficiency, <100mb RSS per active engine for any realistic project.
- Reasonably fast indexing and ~instant startup with an existing index. Launching an
  IDE after you booted your PC shouldn't make you wait a minute or two with CPU going brrr.
- Maintainable. Stupid (in a good way) code is preferred even if it makes it more
  verbose, rather than having overly smart code. It is important that I know the whole
  codebase and can orient myself there.
- Provide enough data for day-to-day work. Normal things should work kind of well;
  it's fine if some things are not implemented as long as users get ~90% of a complete
  LSP.

As a result, the following things are expected:

- Type inference is not expected to be complete.
- Trait solving is not expected to be complete.
- No proc macro support.
- No proactive/interactive build script resolving.
- New or changed module-level things (declarations, impls, derives, imports, and so on) become fully
  available after save. While the file is dirty, we still analyze the current function body using
  the last saved project, but we don't build a second unsaved project in the background.
- Cross-file operations may ask you to save first. They use locations from the saved project, so
  returning them for different open text would be worse than returning nothing.

An additional implication is:

**No unneeded features**. If something is added, it means that it affects a significant
chunk of users. "Good to have because why not" features are not really for this project.

An example of that is unstable nightly features, especially ones that are expected to
change often. _At least_ until the project provides good enough coverage for stable,
we don't want to start working on nightly-only features (_especially_ big ones like const
generics or specialization).

Some of these things _might_ change in the future, but only if they don't sacrifice the
goals stated above.

We already have a complete implementation of Rust LSP -- rust-analyzer. So it's OK for
this project to prioritize something else over completeness; otherwise we will inevitably
end up with a rust-analyzer 2.0, which is explicitly not a goal.
