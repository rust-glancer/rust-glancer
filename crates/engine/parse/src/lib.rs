mod current_source;
mod db;
mod declaration_header;
mod file;
mod fs;
mod line_index;
mod module;
mod package;
mod target;
mod text_range_map;

#[cfg(test)]
mod tests;

pub use self::{
    current_source::CurrentSource,
    db::{PackageFileRef, ParseDb, SavedFileRefresh},
    declaration_header::{DeclarationAssociationIndex, DeclarationHeaderCursor},
    file::{ParsedFile, ParsedFileSnapshot, parse_source_file, syntax_edition},
    line_index::{LineColumnSpan, LineEndings, LineIndex, Position},
    module::{
        ModuleFileContext, ModuleFileResolution, enclosing_inline_module_path, module_path_override,
    },
    package::{Package, PackageParseSnapshot},
    target::{CargoTarget, CargoTargetId},
    text_range_map::{TextRangeMap, TextRangeMapping},
};
