//! Read comments from the query's source, then attach them to saved declarations for link lookup.
//! Comment edits change the Markdown and its source positions without necessarily changing the
//! declaration that supplies the link's scope.

use std::ops::Range;

use anyhow::Context as _;
use rg_ir_model::{CrateRef, FileId, Span, TextSpan, identity::DeclarationRef};
use rg_ir_view::{
    item::documentation::{DocumentationLinkResolution, DocumentationView},
    source::{
        DocumentationDeclarationIndex, DocumentationPlacement, DocumentationSource,
        DocumentationSourceView,
    },
};
use rg_parse::{TextRangeMap, lexical_token_kind_at, parse_source_file};
use rg_syntax::{
    AstNode as _, SourceFile, SyntaxKind, SyntaxNode, TextRange, TextSize,
    ast::{self, HasAttrs as _},
};

use super::MarkdownLink;
use crate::Analysis;

/// Keep a declaration's outer and inner docs together for Markdown parsing.
///
/// A `/// [profile][target]` above `mod api;` can use a reference definition in a `//!` comment
/// in `api.rs`. Those comments form one Markdown document, but each part keeps its own Rust
/// file ranges and placement so links can be displayed and resolved where they were written.
pub(crate) struct SourceDocumentation {
    /// The saved declaration whose scope resolves links. Without a unique association,
    /// examples still have syntax to highlight, but item links cannot be resolved.
    pub(crate) owner: Option<DeclarationRef>,
    pub(crate) text: String,
    parts: Vec<DocumentationPart>,
}

/// One contribution to the joined Markdown, with the information needed to get back to its file.
/// Its mappings use offsets in the part's own Markdown; `markdown` locates that text in the
/// complete document. The text itself belongs to the complete document, so the part only keeps
/// its mappings and the placement that selects a scope for its links.
struct DocumentationPart {
    file: FileId,
    /// Complete byte range in `SourceDocumentation::text`, including any unmapped text.
    markdown: Range<usize>,
    placement: DocumentationPlacement,
    mappings: TextRangeMap,
}

impl SourceDocumentation {
    /// Append a nonempty part and remember where its Markdown starts. The separating newline
    /// belongs only to the joined document, so it has no corresponding Rust source bytes.
    fn push(
        &mut self,
        file: FileId,
        placement: DocumentationPlacement,
        source: DocumentationSource,
    ) {
        if source.text().trim().is_empty() {
            return;
        }
        let (text, mappings) = source.into_parts();
        // Reuse the first buffer. Later parts are copied into it and then released; their
        // mappings need only the part's extent, not a second copy of its Markdown.
        let start = if self.text.is_empty() {
            self.text = text;
            0
        } else {
            self.text.push('\n');
            let start = self.text.len();
            self.text.push_str(&text);
            start
        };
        self.parts.push(DocumentationPart {
            file,
            markdown: start..self.text.len(),
            placement,
            mappings,
        });
    }

    /// Find whether a byte offset in the joined Markdown came from outer or inner docs.
    /// Separators inserted between parts have no placement. A link uses its starting part
    /// to select the scope, even when a reference definition is written in another part.
    pub(crate) fn placement_at(&self, offset: usize) -> Option<DocumentationPlacement> {
        self.parts
            .iter()
            .find(|part| part.markdown.contains(&offset))
            .map(|part| part.placement)
    }

    /// Translate a byte range in the joined Markdown into spans in the requested Rust file.
    /// Text from other files contributes no spans here. A multiline link or example may cross
    /// comment prefixes, so the result keeps those source fragments separate.
    pub(crate) fn source_ranges(&self, file: FileId, range: Range<usize>) -> Vec<Span> {
        let ranges = self
            .parts
            .iter()
            .filter(|part| part.file == file)
            .flat_map(|part| {
                let start = range.start.saturating_sub(part.markdown.start);
                let end = range
                    .end
                    .saturating_sub(part.markdown.start)
                    .min(part.markdown.len());
                part.mappings.project(start..end).map(|mapping| {
                    Span::from_text_range(TextRange::new(
                        TextSize::try_from(mapping.original.start)
                            .expect("source range fits syntax offsets"),
                        TextSize::try_from(mapping.original.end)
                            .expect("source range fits syntax offsets"),
                    ))
                })
            });
        // A decoded escape may split one authored link into adjacent mappings. Join those
        // pieces again for presentation while keeping gaps such as comment prefixes separate.
        let mut spans: Vec<Span> = Vec::new();
        for span in ranges {
            if let Some(last) = spans.last_mut()
                && last.text.end == span.text.start
            {
                last.text.end = span.text.end;
            } else {
                spans.push(span);
            }
        }
        spans
    }

    /// Locate a Rust file byte offset in the joined Markdown by going through its owning part.
    /// Positions outside the retained comment text, such as `///` prefixes, have no match.
    fn markdown_offset(&self, file: FileId, offset: u32) -> Option<usize> {
        self.parts
            .iter()
            .filter(|part| part.file == file)
            .find_map(|part| {
                part.mappings
                    .generated_offset(offset as usize)
                    .map(|offset| offset + part.markdown.start)
            })
    }
}

/// A resolved declaration and the source fragment of its link under the cursor.
/// A link spanning several comments uses only the fragment containing the cursor as its span.
pub(crate) struct SourceDocumentationLink {
    pub(crate) declaration: DeclarationRef,
    pub(crate) span: Span,
}

/// Use the request's syntax for comment text and positions, and saved declarations for scope.
/// Matching the documented item's header connects those two views when comments have been edited.
pub(crate) struct SourceDocumentationQuery<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> SourceDocumentationQuery<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    /// Locate one authored link, then use exactly the same path resolver as rendered hover docs.
    /// The input offset and returned span are in the request's Rust file; Markdown positions
    /// are used only while finding and resolving the link inside its complete document.
    pub(crate) fn link_at(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<SourceDocumentationLink>> {
        let Some(syntax) = self
            .syntax_for_query(crate_ref, file, Some(offset))
            .context("read documentation syntax")?
        else {
            return Ok(None);
        };
        // Ordinary code queries should not enumerate documentation or resolve any links.
        if !syntax
            .syntax()
            .token_at_offset(offset.into())
            .into_iter()
            .any(|token| {
                token.kind() == SyntaxKind::COMMENT
                    || token
                        .parent_ancestors()
                        .filter_map(ast::Attr::cast)
                        .any(|attr| attr.simple_name().as_deref() == Some("doc"))
            })
        {
            return Ok(None);
        }
        for docs in self
            .documents(
                crate_ref,
                file,
                &syntax,
                Some(TextSpan {
                    start: offset,
                    end: offset.saturating_add(1),
                }),
            )
            .context("read source documentation")?
        {
            let Some(markdown_offset) = docs.markdown_offset(file, offset) else {
                continue;
            };
            for link in MarkdownLink::extract(&docs.text)
                .filter(|link| link.range.contains(&markdown_offset))
            {
                if let Some(declaration) = self
                    .resolve(&docs, &link)
                    .context("resolve source documentation link")?
                    && let Some(span) = docs
                        .source_ranges(file, link.range)
                        .into_iter()
                        .find(|span| span.contains(offset))
                {
                    return Ok(Some(SourceDocumentationLink { declaration, span }));
                }
            }
        }
        Ok(None)
    }

    /// Resolve a link using its documented owner and the placement of its first Markdown byte.
    /// Reference definitions supply the path spelling, but the link's own part selects the scope.
    /// An unknown owner, unresolved path, or ordinary Markdown URL supplies no declaration.
    pub(crate) fn resolve(
        &self,
        docs: &SourceDocumentation,
        link: &MarkdownLink<'_>,
    ) -> anyhow::Result<Option<DeclarationRef>> {
        let (Some(owner), Some(placement)) = (docs.owner, docs.placement_at(link.range.start))
        else {
            return Ok(None);
        };
        Ok(
            match DocumentationView::new(self.0.view_db())
                .resolve_link(owner, placement, &link.destination)
                .context("resolve authored item path")?
            {
                DocumentationLinkResolution::Declaration(declaration) => Some(declaration),
                DocumentationLinkResolution::Unresolved
                | DocumentationLinkResolution::NotAnItemLink => None,
            },
        )
    }

    /// Reuse captured editor syntax, or parse saved text when the request has no syntax for it.
    /// Before parsing for a link query, check whether its cursor could be in documentation.
    /// `None` can mean syntax is unavailable or that the cursor made parsing unnecessary.
    /// Highlighting and counterpart docs need the whole file and pass no cursor.
    pub(crate) fn syntax_for_query(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        link_offset: Option<u32>,
    ) -> anyhow::Result<Option<SourceFile>> {
        let edition = self
            .0
            .view_db()
            .crate_edition(crate_ref)
            .context("read documentation edition")?;
        if let Some(source) = self.0.current_source(crate_ref.package, file) {
            return Ok(source.parse(edition).map(|parsed| parsed.tree()));
        }
        let Some(text) = self
            .0
            .saved_source_text_for_file(crate_ref.package, file)
            .context("read saved documentation source")?
        else {
            return Ok(None);
        };
        // Saved files have no retained syntax tree. Lexing is enough to rule out ordinary
        // code without constructing one on every hover or navigation request. Use Rust tokens
        // so multiline comments and raw doc strings work too; the syntax check in `link_at`
        // still decides whether a string belongs to a doc attribute.
        if let Some(offset) = link_offset {
            let kind = lexical_token_kind_at(&text, edition, offset);
            if !matches!(kind, Some(SyntaxKind::COMMENT | SyntaxKind::STRING)) {
                return Ok(None);
            }
        }
        Ok(Some(parse_source_file(&text, edition).tree()))
    }

    /// Select comments touching the requested source range, then assemble their complete documents.
    /// Even a request for a small range needs the rest of each document: `[profile][target]` may
    /// refer to a definition outside that range, or in the module's other file. The requested
    /// range selects documents; it must not truncate their Markdown before parsing.
    pub(crate) fn documents(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        syntax: &SourceFile,
        range: Option<TextSpan>,
    ) -> anyhow::Result<Vec<SourceDocumentation>> {
        let view = DocumentationSourceView::new(self.0.view_db());
        let mut names = None;
        let mut documents = Vec::new();
        for node in syntax.syntax().descendants() {
            rg_std::check_cancel!(self.0, "source documentation owner");
            let is_file = SourceFile::can_cast(node.kind());
            let item = ast::Item::cast(node.clone());
            if !is_file
                && item.is_none()
                && !ast::Variant::can_cast(node.kind())
                && !ast::RecordField::can_cast(node.kind())
                && !ast::TupleField::can_cast(node.kind())
            {
                continue;
            }
            // Skip unrelated items before copying their comments. A containing module still
            // needs the more precise check below: its own docs may be outside this range.
            if range.is_some_and(|range| {
                let span = Span::from_text_range(node.text_range());
                span.text.start >= range.end || span.text.end <= range.start
            }) {
                continue;
            }
            let outer = DocumentationSource::from_node(&node, DocumentationPlacement::Outer);
            let inner = if is_file {
                Some(node.clone())
            } else {
                item.and_then(|item| item.inner_attributes_node())
            }
            .map(|node| DocumentationSource::from_node(&node, DocumentationPlacement::Inner));
            if outer.text().trim().is_empty()
                && inner
                    .as_ref()
                    .is_none_or(|docs| docs.text().trim().is_empty())
            {
                continue;
            }
            // Check only the parts authored here before associating an owner or opening another
            // file. An unrelated `mod api;` must not make a small query parse all of `api.rs`.
            if let Some(range) = range {
                let range = range.start as usize..range.end as usize;
                if !outer.intersects_source_range(range.clone())
                    && inner
                        .as_ref()
                        .is_none_or(|docs| !docs.intersects_source_range(range.clone()))
                {
                    continue;
                }
            }
            // File-level inner docs use the file's module directly. Item docs need their
            // current header associated with a saved declaration before links have a scope.
            let owner = if is_file {
                view.file_module(crate_ref, file)
                    .context("find documented file module")?
                    .map(DeclarationRef::Module)
            } else {
                let names = if let Some(names) = &names {
                    names
                } else {
                    names.insert(
                        view.declaration_index(crate_ref, file)
                            .context("read documentation declarations")?,
                    )
                };
                self.owner(crate_ref, file, &node, names)
                    .context("associate documentation owner")?
            };
            let mut docs = SourceDocumentation {
                owner,
                text: String::new(),
                parts: Vec::new(),
            };
            // Join outer then inner docs, even when an outline module puts them in two files.
            // Their source placement stays attached, so edited Markdown cannot change scope.
            if is_file {
                self.append_module_counterpart(
                    &mut docs,
                    crate_ref,
                    file,
                    DocumentationPlacement::Outer,
                )
                .context("read module outer documentation")?;
            } else {
                docs.push(file, DocumentationPlacement::Outer, outer);
            }
            if let Some(inner) = inner {
                docs.push(file, DocumentationPlacement::Inner, inner);
            } else {
                self.append_module_counterpart(
                    &mut docs,
                    crate_ref,
                    file,
                    DocumentationPlacement::Inner,
                )
                .context("read module inner documentation")?;
            }
            documents.push(docs);
        }
        Ok(documents)
    }

    /// A reference definition in the other module file still belongs to this Markdown document.
    /// When reading `api.rs`, fetch outer docs from `mod api;`; when reading that declaration,
    /// fetch inner docs from `api.rs`. Inline modules already have both parts in the same file.
    fn append_module_counterpart(
        &self,
        docs: &mut SourceDocumentation,
        crate_ref: CrateRef,
        file: FileId,
        placement: DocumentationPlacement,
    ) -> anyhow::Result<()> {
        let Some(DeclarationRef::Module(module)) = docs.owner else {
            return Ok(());
        };
        let Some(source) = DocumentationSourceView::new(self.0.view_db())
            .module_source(module)
            .context("read module source parts")?
        else {
            return Ok(());
        };
        let (other, declaration_span) = match placement {
            DocumentationPlacement::Outer => {
                let Some((other, span)) = source.declaration else {
                    return Ok(());
                };
                (other, Some(span))
            }
            DocumentationPlacement::Inner => {
                let Some(other) = source.definition else {
                    return Ok(());
                };
                (other, None)
            }
        };
        if other == file {
            return Ok(());
        }
        let Some(syntax) = self
            .syntax_for_query(crate_ref, other, None)
            .context("read counterpart syntax")?
        else {
            return Ok(());
        };
        let node = if let Some(span) = declaration_span {
            syntax.syntax().descendants().find(|node| {
                ast::Module::can_cast(node.kind())
                    && Span::from_text_range(node.text_range()) == span
            })
        } else {
            Some(syntax.syntax().clone())
        };
        if let Some(node) = node {
            docs.push(
                other,
                placement,
                DocumentationSource::from_node(&node, placement),
            );
        }
        Ok(())
    }

    /// Find the saved declaration that supplies this syntax node's documentation scope.
    /// Editing a comment can move every name below it, so first map a current header token
    /// into saved coordinates, then look it up in the saved name inventory. An unavailable or
    /// ambiguous association leaves the owner unknown instead of borrowing a nearby item's scope.
    fn owner(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        node: &SyntaxNode,
        names: &DocumentationDeclarationIndex,
    ) -> anyhow::Result<Option<DeclarationRef>> {
        // Enum fields do not have their own DeclarationRef. Their variant supplies the same
        // module and `Self` scope, so use that indexed owner without inventing a field identity.
        let variant = (ast::RecordField::can_cast(node.kind())
            || ast::TupleField::can_cast(node.kind()))
        .then(|| {
            node.ancestors()
                .find(|node| ast::Variant::can_cast(node.kind()))
        })
        .flatten();
        let node = variant.as_ref().unwrap_or(node);
        let name_offset = node
            .children()
            .find_map(ast::Name::cast)
            .map(|name| name.syntax().text_range().start())
            .or_else(|| {
                ast::TupleField::cast(node.clone())?
                    .ty()
                    .map(|ty| ty.syntax().text_range().start())
            });
        let Some(offset) = name_offset else {
            return Ok(None);
        };
        let Some(saved) = self
            .0
            .saved_header_offset_for_current(crate_ref, file, offset.into())
            .context("map documentation owner to saved header")?
        else {
            return Ok(None);
        };
        Ok(names.declaration_at(saved))
    }
}
