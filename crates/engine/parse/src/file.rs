use anyhow::Context as _;
use std::{
    borrow::Cow,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use rg_arena::Arena;
use rg_syntax::{Edition, Parse as SyntaxParse, SourceFile};
use rg_workspace::RustEdition;

use crate::{
    line_index::{LineIndex, LineIndexSnapshot},
    span::Span,
};

/// Stable identifier for a parsed source file inside `FileDb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
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
    /// Source backing used when syntax has to be rebuilt after eviction.
    pub(crate) source: ParsedSource,
    /// Line-start index used to convert byte offsets into line/column coordinates.
    pub(crate) line_index: LineIndexState,
    /// Rust parse result produced by `rg_syntax`.
    ///
    /// This is retained only while AST-consuming phases are lowering. Query-time state keeps
    /// paths and line indexes, but can evict parse trees to keep memory bounded.
    ///
    /// `rg_syntax::SourceFile` is a traversal cursor over an immutable parse tree. Keeping the
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
    edition: RustEdition,
    data: &'a ParsedFileData,
}

/// Source backing for a parsed file whose syntax tree may be evicted.
#[derive(Debug, Clone)]
pub(crate) enum ParsedSource {
    SavedFile,
    InMemory(Arc<str>),
}

/// Serializable file metadata retained after syntax trees are evicted.
///
/// Cache-backed startup restores this data so later queries can still translate file ids into
/// paths and source coordinates without rebuilding item trees first.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct ParsedFileSnapshot {
    pub(crate) path: ParsedFilePath,
    pub(crate) line_index: LineIndexSnapshot,
}

impl ParsedFileSnapshot {
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }
}

/// Cache-friendly file path representation for parse snapshots.
///
/// `PathBuf` is the natural in-memory type, but cache artifacts should stay platform-neutral. The
/// snapshot stores the canonical path as a string and converts back to `PathBuf` when restoring the
/// parse database.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub(crate) struct ParsedFilePath(pub(crate) String);

impl ParsedFilePath {
    fn from_path(path: &Path) -> Self {
        Self(path.to_string_lossy().into_owned())
    }

    fn into_path_buf(self) -> PathBuf {
        PathBuf::from(self.0)
    }

    fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }
}

impl<'a> ParsedFile<'a> {
    fn new(file_id: FileId, edition: RustEdition, data: &'a ParsedFileData) -> Self {
        Self {
            file_id,
            edition,
            data,
        }
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
    pub fn line_index(&self) -> anyhow::Result<&'a LineIndex> {
        self.data.line_index.get(&self.data.path)
    }

    /// Returns a local syntax cursor over the retained parse tree.
    ///
    /// This does not reparse source text. It creates a fresh typed root over the immutable parse
    /// tree so callers can traverse AST without sharing `rg_syntax` cursor state.
    pub fn syntax(&self) -> Option<SourceFile> {
        self.data.syntax.as_ref().map(|syntax| syntax.tree())
    }

    /// Returns a freshly parsed syntax tree without storing it in the parse database.
    pub fn parse_syntax(&self) -> anyhow::Result<SyntaxParse<SourceFile>> {
        let source = self.data.source.read(&self.data.path)?;
        Ok(FileDb::parse_syntax(&source, self.edition))
    }

    /// Returns source text for a byte span from the same snapshot that backs this parsed file.
    pub fn text_for_span(&self, span: Span) -> Option<String> {
        let file_text = self.data.source.read(&self.data.path).ok()?;
        let start = usize::try_from(span.text.start).ok()?;
        let end = usize::try_from(span.text.end).ok()?;

        file_text.get(start..end).map(ToString::to_string)
    }
}

/// Shared parse cache that owns filesystem-backed source files and syntax trees.
///
/// `FileDb` deduplicates parsing across targets, so shared modules are parsed once
/// and reused during multiple target traversals.
#[derive(Debug, Clone)]
pub(super) struct FileDb {
    pub(crate) edition: RustEdition,
    pub(crate) parsed_files: Arena<FileId, ParsedFileData>,
    pub(crate) file_ids_by_path: HashMap<PathBuf, FileId>,
}

impl FileDb {
    pub(super) fn new(edition: RustEdition) -> Self {
        Self {
            edition,
            parsed_files: Arena::new(),
            file_ids_by_path: HashMap::new(),
        }
    }

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

        let file_id = self.parsed_files.alloc(Self::parse_source(
            canonical_file_path.clone(),
            &source,
            self.edition,
            ParsedSource::SavedFile,
        ));
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
        self.parsed_files[file_id] = Self::parse_source(
            file_path.to_path_buf(),
            &source,
            self.edition,
            ParsedSource::SavedFile,
        );
        Ok(Some(file_id))
    }

    /// Reparses an already known file from caller-provided source text.
    pub(super) fn reparse_file_from_source(
        &mut self,
        file_path: &Path,
        source: Arc<str>,
    ) -> Option<FileId> {
        let file_id = self.file_ids_by_path.get(file_path).copied()?;
        let source_text = source.as_ref();
        let source_backing = ParsedSource::InMemory(Arc::clone(&source));
        self.parsed_files[file_id] = Self::parse_source(
            file_path.to_path_buf(),
            source_text,
            self.edition,
            source_backing,
        );
        Some(file_id)
    }

    /// Ensures that syntax for an already known file is available for AST-consuming lowering.
    pub(super) fn ensure_file_syntax(&mut self, file_id: FileId) -> anyhow::Result<()> {
        let Some(parsed_file) = self.parsed_files.get(file_id) else {
            anyhow::bail!("unknown file id {:?}", file_id);
        };
        if parsed_file.syntax.is_some() {
            return Ok(());
        }

        let source = parsed_file.source.read(&parsed_file.path)?;
        let path = parsed_file.path.clone();
        let source_backing = parsed_file.source.clone();
        self.parsed_files[file_id] =
            Self::parse_source(path, &source, self.edition, source_backing);
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
            if let Ok(line_index) = parsed_file.line_index.get_mut(&parsed_file.path) {
                indexes.push(line_index);
            }
        }
    }

    pub(super) fn offload_line_indexes(&mut self) {
        for parsed_file in self.parsed_files.iter_mut() {
            if matches!(parsed_file.source, ParsedSource::SavedFile) {
                parsed_file.line_index.offload();
            }
        }
    }

    /// Returns the cached parsed file for a previously known `FileId`.
    pub(super) fn parsed_file(&self, file_id: FileId) -> Option<ParsedFile<'_>> {
        self.parsed_files
            .get(file_id)
            .map(|data| ParsedFile::new(file_id, self.edition, data))
    }

    /// Returns all cached parsed files.
    pub(super) fn parsed_files(&self) -> impl Iterator<Item = ParsedFile<'_>> {
        self.parsed_files
            .iter_with_ids()
            .map(|(file_id, data)| ParsedFile::new(file_id, self.edition, data))
    }

    pub(super) fn parse_snapshot(&self) -> anyhow::Result<Vec<ParsedFileSnapshot>> {
        self.parsed_files
            .iter()
            .map(ParsedFileData::parse_snapshot)
            .collect()
    }

    pub(super) fn from_parse_snapshot(
        edition: RustEdition,
        files: Vec<ParsedFileSnapshot>,
    ) -> Self {
        let parsed_files = Arena::from_vec(
            files
                .into_iter()
                .map(ParsedFileData::from_parse_snapshot)
                .collect::<Vec<_>>(),
        );
        let file_ids_by_path = parsed_files
            .iter_with_ids()
            .map(|(file_id, file)| (file.path.clone(), file_id))
            .collect::<HashMap<_, _>>();

        Self {
            edition,
            parsed_files,
            file_ids_by_path,
        }
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

    fn parse_source(
        path: PathBuf,
        source: &str,
        edition: RustEdition,
        source_backing: ParsedSource,
    ) -> ParsedFileData {
        let line_index = LineIndex::new(source);
        let parsed_file = Self::parse_syntax(source, edition);

        ParsedFileData {
            path,
            source: source_backing,
            line_index: LineIndexState::resident(line_index),
            syntax: Some(parsed_file),
        }
    }

    fn parse_syntax(source: &str, edition: RustEdition) -> SyntaxParse<SourceFile> {
        let ra_edition = match edition {
            RustEdition::Edition2015 => Edition::Edition2015,
            RustEdition::Edition2018 => Edition::Edition2018,
            RustEdition::Edition2021 => Edition::Edition2021,
            RustEdition::Edition2024 => Edition::Edition2024,
        };
        SourceFile::parse(source, ra_edition)
    }
}

impl ParsedSource {
    fn read<'a>(&'a self, path: &Path) -> anyhow::Result<Cow<'a, str>> {
        match self {
            Self::SavedFile => Ok(Cow::Owned(FileDb::read_source(path)?)),
            Self::InMemory(source) => Ok(Cow::Borrowed(source.as_ref())),
        }
    }
}

impl ParsedFileData {
    fn parse_snapshot(&self) -> anyhow::Result<ParsedFileSnapshot> {
        Ok(ParsedFileSnapshot {
            path: ParsedFilePath::from_path(&self.path),
            line_index: self.line_index.get(&self.path)?.to_snapshot(),
        })
    }

    fn from_parse_snapshot(snapshot: ParsedFileSnapshot) -> Self {
        Self {
            path: snapshot.path.into_path_buf(),
            source: ParsedSource::SavedFile,
            line_index: LineIndexState::resident(LineIndex::from_snapshot(snapshot.line_index)),
            syntax: None,
        }
    }

    fn shrink_to_fit(&mut self) {
        self.line_index.shrink_to_fit();
    }
}

/// Lazily resident source map for a parsed file.
///
/// File paths and file ids stay resident because they define package inventory. The heavier line
/// tables can be dropped after package artifacts are durable and reconstructed from the saved source
/// file when an LSP range conversion actually needs them.
#[derive(Debug)]
pub(crate) enum LineIndexState {
    Resident(LineIndex),
    Offloaded(OnceLock<LineIndex>),
}

impl LineIndexState {
    fn resident(line_index: LineIndex) -> Self {
        Self::Resident(line_index)
    }

    fn get(&self, path: &Path) -> anyhow::Result<&LineIndex> {
        match self {
            Self::Resident(line_index) => Ok(line_index),
            Self::Offloaded(line_index) => {
                if let Some(line_index) = line_index.get() {
                    return Ok(line_index);
                }

                let source = FileDb::read_source(path)?;
                let _ = line_index.set(LineIndex::new(&source));
                Ok(line_index
                    .get()
                    .expect("offloaded line index should be initialized after successful load"))
            }
        }
    }

    fn get_mut(&mut self, path: &Path) -> anyhow::Result<&mut LineIndex> {
        if matches!(self, Self::Offloaded(_)) {
            let source = FileDb::read_source(path)?;
            *self = Self::Resident(LineIndex::new(&source));
        }

        match self {
            Self::Resident(line_index) => Ok(line_index),
            Self::Offloaded(_) => unreachable!("offloaded line index was made resident above"),
        }
    }

    fn offload(&mut self) {
        *self = Self::Offloaded(OnceLock::new());
    }

    fn shrink_to_fit(&mut self) {
        if let Ok(line_index) = self.get_mut_without_loading() {
            line_index.shrink_to_fit();
        }
    }

    fn get_mut_without_loading(&mut self) -> Result<&mut LineIndex, ()> {
        match self {
            Self::Resident(line_index) => Ok(line_index),
            Self::Offloaded(line_index) => line_index.get_mut().ok_or(()),
        }
    }
}

impl Clone for LineIndexState {
    fn clone(&self) -> Self {
        match self {
            Self::Resident(line_index) => Self::Resident(line_index.clone()),
            Self::Offloaded(line_index) => {
                let cloned = OnceLock::new();
                if let Some(line_index) = line_index.get() {
                    let _ = cloned.set(line_index.clone());
                }
                Self::Offloaded(cloned)
            }
        }
    }
}
