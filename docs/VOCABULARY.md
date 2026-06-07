# Project vocabulary

This project uses a set of terms and naming conventions that are expected to be internally
consistent, but do not necessarily match the strict formal definitions used elsewhere.

This document aims to provide better intel on how these are used.

## Object families

There are several consistent "families" identified by their suffix:

- `*Id`: object indices for arenas, used for lookup.
- `*Ref`: stable references to objects across storage boundaries. A ref usually carries enough
  origin context to route lookup to the right store before applying the underlying id.
- `*Data`: stored shape of an entity. Data answers "what is this thing?", owns the structure
  needed for traversal or lookup, and should be valid immediately after the entity is lowered or
  collected.
- `*Facts`: pass-derived knowledge attached to an existing entity. Facts answer "what have later
  phases concluded about this thing?", can usually be recomputed from data plus surrounding
  context, and may start as `Unknown` until the relevant pass fills them.
- `*View`: read-only projection over stored or indexed data. Views are usually query-facing and
  may combine several lower-level stores into the shape needed by downstream analysis.
- `*Store`: owning indexed storage for a phase, target, or lexical boundary. Stores usually hold
  arenas, maps, or other lookup tables and are the main place where ids become addressable data.
- `*Query`: algorithm object that carries the sources and context needed to answer one class of
  lookup question. Queries keep routing and temporary resolution state close to the operation.
- `*Builder`: mutable construction-phase counterpart to frozen data or stores. Builders collect,
  allocate, and connect entities before producing the immutable shape used by later phases.
- `*Signature`: compact declaration header retained for semantic queries and display. Signatures
  preserve the parts of an item header that affect type, navigation, hover, or completion behavior.
- `*Resolution`: selected target or targets of name, path, or type lookup. Resolution values
  describe what lookup concluded, including explicit unknown or ambiguous shapes when needed.
- `Resolve*Result`: richer result shape returned by a resolution algorithm. These results may
  include the selected targets together with traversal metadata, partial-resolution state, or
  failure position details.
