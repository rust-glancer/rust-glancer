use anyhow::Context as _;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use ra_syntax::{Edition, Parse as SyntaxParse, SourceFile};
use rg_arena::Arena;

use crate::span::{LineIndex, Span};

/// Stable identifier for a parsed source file inside `FileDb`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct FileId(pub usize);

impl rg_arena::ArenaId for FileId {
    fn from_index(index: usize) -> Self {
        Self(index)
    }

    fn index(self) -> usize {
        self.0
    }
}

/// Internal parsed representation used by the parser cache.
#[derive(Debug, Clone)]
pub(crate) struct ParsedFileData {
    /// Canonical filesystem path for this source file.
    pub(crate) path: PathBuf,
    /// Line-start index used to convert byte offsets into line/column coordinates.
    pub(crate) line_index: LineIndex,
    /// Green-backed Rust parse result produced by `ra_syntax`.
    ///
    /// This is retained only while AST-consuming phases are lowering. Query-time state keeps
    /// paths and line indexes, but can evict parse trees to keep memory bounded.
    ///
    /// `ra_syntax::SourceFile` is a traversal cursor over this immutable green tree. Keeping the
    /// parse result lets each AST-consuming phase create a fresh local cursor instead of sharing
    /// cursor internals across package or thread boundaries.
    pub(crate) syntax: Option<SyntaxParse<SourceFile>>,
}

/// Borrowed view over one cached source file.
///
/// Later phases need syntax and source coordinates, but they should not know that parsing is backed
/// by a mutable file cache. This view is the stable boundary between `parse` and AST-consuming
/// phases.
#[derive(Debug, Clone, Copy)]
pub struct ParsedFile<'a> {
    file_id: FileId,
    data: &'a ParsedFileData,
}

impl<'a> ParsedFile<'a> {
    fn new(file_id: FileId, data: &'a ParsedFileData) -> Self {
        Self { file_id, data }
    }

    /// Returns the stable package-local id for this parsed source file.
    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    /// Returns the canonical path for this parsed source file.
    pub fn path(&self) -> &'a Path {
        self.data.path.as_path()
    }

    /// Returns the line index used for byte-offset to line/column conversion.
    pub fn line_index(&self) -> &'a LineIndex {
        &self.data.line_index
    }

    /// Returns a local syntax cursor over the retained parse tree.
    ///
    /// This does not reparse source text. It creates a fresh typed root over the immutable green
    /// tree so callers can traverse AST without sharing `ra_syntax` cursor state.
    pub fn syntax(&self) -> Option<SourceFile> {
        self.data.syntax.as_ref().map(|syntax| syntax.tree())
    }

    /// Returns source text for a byte span by reading it from the saved source file.
    pub fn text_for_span(&self, span: Span) -> Option<String> {
        let file_text = std::fs::read_to_string(&self.data.path).ok()?;
        let start = usize::try_from(span.text.start).ok()?;
        let end = usize::try_from(span.text.end).ok()?;

        file_text.get(start..end).map(ToString::to_string)
    }
}

/// Shared parse cache that owns filesystem-backed source files and syntax trees.
///
/// `FileDb` deduplicates parsing across targets, so shared modules are parsed once
/// and reused during multiple target traversals.
#[derive(Default, Debug, Clone)]
pub(super) struct FileDb {
    pub(crate) parsed_files: Arena<FileId, ParsedFileData>,
    pub(crate) file_ids_by_path: HashMap<PathBuf, FileId>,
}

impl FileDb {
    /// Returns an existing `FileId` for `file_path` or parses and caches the file.
    pub(super) fn get_or_parse_file(&mut self, file_path: &Path) -> anyhow::Result<FileId> {
        let canonical_file_path = file_path
            .canonicalize()
            .with_context(|| format!("while attempting to canonicalize {}", file_path.display()))?;

        if let Some(file_id) = self.file_ids_by_path.get(&canonical_file_path).copied() {
            self.ensure_file_syntax(file_id)?;
            return Ok(file_id);
        }

        let source = Self::read_source(&canonical_file_path)?;

        let file_id = self
            .parsed_files
            .alloc(Self::parse_source(canonical_file_path.clone(), &source));
        self.file_ids_by_path.insert(canonical_file_path, file_id);

        Ok(file_id)
    }

    /// Reparses an already known file from the saved filesystem snapshot.
    pub(super) fn reparse_file_from_disk(
        &mut self,
        file_path: &Path,
    ) -> anyhow::Result<Option<FileId>> {
        let Some(file_id) = self.file_ids_by_path.get(file_path).copied() else {
            return Ok(None);
        };

        let source = Self::read_source(file_path)?;
        self.parsed_files[file_id] = Self::parse_source(file_path.to_path_buf(), &source);
        Ok(Some(file_id))
    }

    /// Ensures that syntax for an already known file is available for AST-consuming lowering.
    pub(super) fn ensure_file_syntax(&mut self, file_id: FileId) -> anyhow::Result<()> {
        let Some(parsed_file) = self.parsed_files.get(file_id) else {
            anyhow::bail!("unknown file id {:?}", file_id);
        };
        if parsed_file.syntax.is_some() {
            return Ok(());
        }

        let source = Self::read_source(&parsed_file.path)?;
        let path = parsed_file.path.clone();
        self.parsed_files[file_id] = Self::parse_source(path, &source);
        Ok(())
    }

    /// Drops retained syntax trees while keeping source coordinates and file identity.
    pub(super) fn evict_syntax_trees(&mut self) {
        for parsed_file in self.parsed_files.iter_mut() {
            parsed_file.syntax = None;
        }
    }

    pub(super) fn shrink_to_fit(&mut self) {
        self.parsed_files.shrink_to_fit();
        self.file_ids_by_path.shrink_to_fit();
        for parsed_file in self.parsed_files.iter_mut() {
            parsed_file.shrink_to_fit();
        }
    }

    pub(super) fn collect_line_indexes<'a>(&'a mut self, indexes: &mut Vec<&'a mut LineIndex>) {
        for parsed_file in self.parsed_files.iter_mut() {
            indexes.push(&mut parsed_file.line_index);
        }
    }

    /// Returns the cached parsed file for a previously known `FileId`.
    pub(super) fn parsed_file(&self, file_id: FileId) -> Option<ParsedFile<'_>> {
        self.parsed_files
            .get(file_id)
            .map(|data| ParsedFile::new(file_id, data))
    }

    /// Returns all cached parsed files.
    pub(super) fn parsed_files(&self) -> impl Iterator<Item = ParsedFile<'_>> {
        self.parsed_files
            .iter_with_ids()
            .map(|(file_id, data)| ParsedFile::new(file_id, data))
    }

    /// Returns the canonical path associated with `file_id`.
    pub(super) fn file_path(&self, file_id: FileId) -> Option<&Path> {
        self.parsed_files
            .get(file_id)
            .map(|parsed_file| parsed_file.path.as_path())
    }

    fn read_source(file_path: &Path) -> anyhow::Result<String> {
        std::fs::read_to_string(file_path)
            .with_context(|| format!("while attempting to read {}", file_path.display()))
    }

    fn parse_source(path: PathBuf, source: &str) -> ParsedFileData {
        let line_index = LineIndex::new(source);
        let parsed_file = SourceFile::parse(source, Edition::CURRENT);

        ParsedFileData {
            path,
            line_index,
            syntax: Some(parsed_file),
        }
    }
}

impl ParsedFileData {
    fn shrink_to_fit(&mut self) {
        self.line_index.shrink_to_fit();
    }
}
