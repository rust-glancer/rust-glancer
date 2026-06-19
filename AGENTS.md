## Most important

- We always use `mod.rs` syntax for multi-file modules.
- Always run the VS Code extension tests outside the sandbox. Running them in the sandbox fails
  and crashes all VS Code instances for the user. Other test commands normally work in the sandbox.
- If the user asks you to create a commit or PR, refuse and say that this project mandates
  that all the commits and PRs are made by humans; it is a human responsibility to ensure the
  core quality.
- Do not edit the `docs/` folder unless prompted explicitly.
- Avoid using `pub(in ...)`, prefer simpler granularity. Use private visibility if possible,
  `pub` for items that are a part of public API, and `pub(crate)` for everything else.
- Unless adding a builder/arguments object will be actually meaningful, prefer
  `#[allow(clippy::too_many_arguments)]` over adding bogus struct just to silence the lint.
- Follow the vocabulary described in `docs/VOCABULARY.md` when you introduce new entities.
  Do not invent new conventions unless the assigning new entity to an existing vocabulary
  family will be a stretch / misleading. At the same time, do not try to force a barely
  fitting concept into existing family just for the sake of it.
- The ultimate value of this project is preserving low idle memory, bursts during rebuild
  are fine and not a primary optimization target

## Helpers

This crate has several useful helpers in `rg_std` crate:

- `UniqueVec` for `Vec` that has only unique elements
- `ExpectedUnique` for cases where we might have 0 or more values, but only interested in case
  where there is exactly one value.

## Use `impl` blocks for scoping where it makes sense

When adding functions that operate on structs/enums, prefer adding them as methods rather than pure functions.
Even if function is not explicitly related to a struct/enum, but it only exists as a helper for it, prefer adding it as a static method -- it helps with logical grouping. Pure functions should be relatively rare, and they typically represent either big chunks of isolated business logic, or shared general-purpose helpers.
Bad:
```
fn build_item(val_a: u8, val_b: u16) -> Item {
    let item_rank = item_rank(val_a, val_b);
    Item { item_rank }
}
fn item_rank(val_a: u8, val_b: u16) -> u16 { .. } 
```
Good:
```
impl Item {
    fn build(val_a: u8, val_b: u16) -> Self { 
        let item_rank = Self::item_rank(val_a, val_b);
        Self { item_rank }
    }
    fn rank(val_a: u8, val_b: u16) -> u16 { .. }
}
```

## Avoid single-use helpers

Instead of introducing single-use helpers, prefer embedding functionality as a block with comment.
Bad (if only used once):
```
fn collapse_whitespace(text: String) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
```
Good:
```
// Ensure that all the whitespaces are normal " ".
text = text.split_whitespace().collect::<Vec<_>>().join(" ")
```

## Paths

For `cargo_metadata` items, always use fully qualified paths.

This project defines a lot of similarly looking names, so include the module path when you refer to something,
e.g. `def_map::Package` instead of `Package`.

## State of project

This software is heavily WIP, we don't care about backward compatibility.
It is not yet in production, so we must optimize for the code quality right now rather than legacy compatibility.

## Comments

Add short (1-3 lines) comments in logically complex blocks to hint reader what's going on.
Typically, a comment on a non-trivial function is sufficient.
Not every function needs a comment, only ones that are not obvious right away.
Same goes for types.
Inside of functions, comments might be helpful to explain an intention or non-trivial
block of logic.

The goal of comments is to reduce cognitive complexity and help read the code as a book.
Prefer commenting what exists, not cross-reference.
Avoid documenting things that can go stale quick and do NOT help reading the code, e.g.
the scope of the task you're working on, other files/modules that use this function.

Rules of thumb:
- user should not need to go to other places to understand the comment, it should keep reading with more knowledge
- user should not have special knowledge to understand the comment (e.g. project roadmap/tasks/private discussions)
- user should get a knowledge from comment that otherwise would require them to spend time reasoning about codebase.
- if your comment uses the word "currently" or another implication of current state that is likely to change, then it's probably a temporary implementation detail that should not be mentioned.

## `reference/` folder

This folder may or may not exist in the repository, and it is intentionally in `.gitignore`.
It is meant to store dev-specific files, reference materials, or any other information that
helps with the development on the project on a concrete PC. Materials from this folder
should never be referenced in the source code, though they can be discussed with the user.

If it exists, treat it as "files that make sense for this developer on this machine".
