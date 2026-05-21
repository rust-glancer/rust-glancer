//! Structural walkers over Body IR syntax.
//!
//! These helpers know how to move through local IR shapes, but they do not decide what a visited
//! node means for resolution, completion, or navigation. Query code should keep that policy close
//! to the query and use these walkers only for the reusable child traversal.

mod pat;
mod path;
mod ty;

pub(crate) use self::{
    pat::{PatWalkSite, walk_pat},
    path::walk_body_path_type_refs,
    ty::walk_type_ref_paths,
};
