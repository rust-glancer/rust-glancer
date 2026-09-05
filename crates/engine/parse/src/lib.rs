mod current_source;
mod db;
mod declaration_header;
mod file;
mod fs;
mod line_index;
mod module;
mod package;
mod span;
mod target;

#[cfg(test)]
mod tests;

pub use self::{
    current_source::CurrentSource,
    db::{PackageFileRef, ParseDb, SavedFileRefresh},
    declaration_header::{DeclarationAssociationIndex, DeclarationHeaderCursor},
    file::{FileId, ParsedFile, ParsedFileSnapshot, parse_source_file},
    line_index::{LineEndings, LineIndex},
    module::{
        ModuleFileContext, ModuleFileResolution, enclosing_inline_module_path, module_path_override,
    },
    package::{Package, PackageParseSnapshot},
    span::{LineColumnSpan, Position, Span, TextSpan},
    target::{CargoTarget, CargoTargetId},
};
