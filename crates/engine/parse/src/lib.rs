//! Rust syntax, source coordinates, and module-file rules.
//!
//! Saved text belongs to `rg_source`; parsing borrows that capture for one operation.
//! Callers supply the edition for their crate interpretation. This crate owns neither
//! workspace metadata nor the mapping from physical files to crates.

mod current_source;
mod declaration_header;
mod file;
mod fs;
mod line_index;
mod module;
mod query;
mod source_reader;
mod text_range_map;

pub use self::{
    current_source::CurrentSource,
    declaration_header::{DeclarationAssociationIndex, DeclarationHeaderCursor},
    file::{is_documentation_token, lexical_token_kind_at, parse_source_file, syntax_edition},
    fs::resolve_path_literal,
    line_index::{LineColumnSpan, LineEndings, LineIndex, Position},
    module::{
        ModuleFileContext, ModuleFileResolution, enclosing_inline_module_path, module_path_override,
    },
    query::saved_syntax,
    source_reader::SavedSourceReader,
    text_range_map::{TextRangeMap, TextRangeMapping},
};
