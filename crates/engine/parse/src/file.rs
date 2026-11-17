use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::{Arc, OnceLock},
};

use rg_ir_model::{FileId, Span};
use rg_source::SourceEntry;
use rg_std::MemorySize;
use rg_syntax::{
    AstNode as _, Edition, LexedStr, Parse as SyntaxParse, SourceFile, SyntaxKind, SyntaxToken, ast,
};
use rg_text::RustEdition;

use crate::line_index::LineIndex;

/// Parse source text with the Rust edition of its owning package.
///
/// This entry point does not assign a project file id or retain the syntax tree. It is intended for
/// editor text that a caller wants to inspect without adding it to the saved parse database.
pub fn parse_source_file(source: &str, edition: RustEdition) -> SyntaxParse<SourceFile> {
    SourceFile::parse(source, syntax_edition(edition))
}

/// Inspect the token at a UTF-8 byte offset without building a syntax tree. Comments and
/// whitespace are included, so callers can decide whether they need to parse the source.
/// Token ranges include their start and exclude their end: a shared boundary selects the
/// following token, and offsets at or beyond the end of the source return `None`.
pub fn lexical_token_kind_at(
    source: &str,
    edition: RustEdition,
    offset: u32,
) -> Option<SyntaxKind> {
    let tokens = LexedStr::new(syntax_edition(edition), source);
    (0..tokens.len())
        .find(|&index| tokens.text_range(index).contains(&(offset as usize)))
        .map(|index| tokens.kind(index))
}

/// Recognize doc comments and every token inside a `doc` attribute through the same AST view.
/// This includes attribute punctuation and escapes, whose spelling is not documentation text.
pub fn is_documentation_token(token: &SyntaxToken) -> bool {
    if token
        .parent_ancestors()
        .filter_map(ast::AnyAttr::cast)
        .any(|attr| match attr {
            ast::AnyAttr::DocComment(_) => true,
            ast::AnyAttr::Attr(attr) => attr.simple_name().as_deref() == Some("doc"),
        })
    {
        return true;
    }

    // Inside macro token trees, doc comments are still bare tokens without attribute nodes.
    matches!(
        token.kind(),
        SyntaxKind::INNER_DOC_COMMENT | SyntaxKind::OUTER_DOC_COMMENT
    )
}

/// Translate the project edition into the parser's edition vocabulary.
pub fn syntax_edition(edition: RustEdition) -> Edition {
    match edition {
        RustEdition::Edition2015 => Edition::Edition2015,
        RustEdition::Edition2018 => Edition::Edition2018,
        RustEdition::Edition2021 => Edition::Edition2021,
        RustEdition::Edition2024 => Edition::Edition2024,
    }
}

/// Source entry and the lazily computed coordinate map.
#[derive(Debug, Clone, MemorySize)]
pub(crate) struct ParsedFileData {
    /// The exact saved revision used by analysis.
    pub(crate) source: Arc<SourceEntry>,
    /// Line-start index used to convert byte offsets into line/column coordinates.
    #[memsize(with = "Self::record_line_index_memory")]
    line_index: OnceLock<LineIndex>,
}

impl ParsedFileData {
    fn record_line_index_memory(
        index: &OnceLock<LineIndex>,
        recorder: &mut rg_std::MemoryRecorder,
    ) {
        if let Some(index) = index.get() {
            index.record_memory_children(recorder);
        }
    }
}

/// Borrowed view over one saved source revision.
#[derive(Debug, Clone, Copy)]
pub struct ParsedFile<'a> {
    file_id: FileId,
    edition: RustEdition,
    data: &'a ParsedFileData,
}

impl<'a> ParsedFile<'a> {
    fn new(file_id: FileId, edition: RustEdition, data: &'a ParsedFileData) -> Self {
        Self {
            file_id,
            edition,
            data,
        }
    }

    /// Returns the canonical ID for this parsed source file.
    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    /// Returns the canonical path for this parsed source file.
    pub fn path(&self) -> &'a Path {
        self.data.source.path()
    }

    /// Returns the line index used for byte-offset to line/column conversion.
    pub fn line_index(&self) -> anyhow::Result<&'a LineIndex> {
        if let Some(index) = self.data.line_index.get() {
            return Ok(index);
        }
        let text = self.data.source.text()?;
        Ok(self.data.line_index.get_or_init(|| LineIndex::new(&text)))
    }

    /// Returns a freshly parsed syntax tree without storing it in the parse database.
    pub fn parse_syntax(&self) -> anyhow::Result<SyntaxParse<SourceFile>> {
        let source = self.data.source.text()?;
        Ok(parse_source_file(&source, self.edition))
    }

    /// Returns source text for a byte span from the same snapshot that backs this parsed file.
    pub fn text_for_span(&self, span: Span) -> anyhow::Result<Option<String>> {
        let file_text = self.data.source.text()?;
        let Ok(start) = usize::try_from(span.start) else {
            return Ok(None);
        };
        let Ok(end) = usize::try_from(span.end) else {
            return Ok(None);
        };

        Ok(file_text.get(start..end).map(ToString::to_string))
    }

    /// Returns source text from the same saved generation backing this parsed file.
    pub fn source_text(&self) -> anyhow::Result<Arc<str>> {
        Ok(self.data.source.text()?)
    }

    /// Returns the strong revision of the exact source bytes backing this parsed file.
    pub fn source_revision(&self) -> rg_source::SourceRevision {
        self.data.source.revision()
    }
}

/// Routes source identities for one package and edition.
#[derive(Debug, Clone, MemorySize)]
pub(super) struct FileDb {
    pub(crate) edition: RustEdition,
    pub(crate) parsed_files: BTreeMap<FileId, ParsedFileData>,
    pub(crate) file_ids_by_path: HashMap<Arc<Path>, FileId>,
}

impl FileDb {
    pub(super) fn new(edition: RustEdition) -> Self {
        Self {
            edition,
            parsed_files: BTreeMap::new(),
            file_ids_by_path: HashMap::new(),
        }
    }

    pub(super) fn insert_captured(&mut self, id: FileId, source: Arc<SourceEntry>) {
        self.file_ids_by_path.insert(source.shared_path(), id);
        self.parsed_files
            .entry(id)
            .or_insert_with(|| ParsedFileData {
                source,
                line_index: OnceLock::new(),
            });
    }

    pub(super) fn parsed_file(&self, file_id: FileId) -> Option<ParsedFile<'_>> {
        self.parsed_files
            .get(&file_id)
            .map(|data| ParsedFile::new(file_id, self.edition, data))
    }

    /// Returns all cached parsed files.
    pub(super) fn parsed_files(&self) -> impl Iterator<Item = ParsedFile<'_>> {
        self.parsed_files
            .iter()
            .map(|(&file_id, data)| ParsedFile::new(file_id, self.edition, data))
    }

    /// Returns the canonical ID if this package already knows the source path.
    pub(super) fn file_id_for_path(&self, file_path: &Path) -> Option<FileId> {
        self.file_ids_by_path.get(file_path).copied()
    }

    pub(super) fn file_path(&self, file_id: FileId) -> Option<&Path> {
        self.parsed_files
            .get(&file_id)
            .map(|parsed_file| parsed_file.source.path())
    }
}
