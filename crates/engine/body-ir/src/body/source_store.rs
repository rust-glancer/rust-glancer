//! Physical occurrences and bounded previews captured while a body's syntax is available.

use std::collections::HashMap;

use rg_def_map::{MacroCopyRef, SourceOccurrenceLocation};
use rg_ir_model::{BodySource, ExprId, SourceId, SourceLocation, SourceLocationKind, Span};
use rg_item_tree::{RecordSource, SourceLocations, SourceRole, TypeSource, initializer_preview};
use rg_std::{MemorySize, Shrink};
use rg_syntax::{AstNode as _, TextRange, ast};
use wincode::{SchemaRead, SchemaWrite};

/// Source data for a single body's occurrence IDs. Nested bodies have their own store.
///
/// All occurrences in this store use owner zero. Unlike a file's declaration table, a body is
/// prepared and compared as a whole, so its expressions and local type syntax can share one
/// recording sequence. Comments and changing byte offsets do not change that sequence.
#[derive(Debug, Default, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodySourceStore {
    entries: Vec<BodySource>,
    initializer_previews: HashMap<SourceId, String>,
    // Only tracked macro arguments need a link to the expansion's copy inventory.
    argument_uses: Vec<MacroArgumentUse>,
    format_captures: Vec<FormatCaptureSource>,
}

impl BodySourceStore {
    pub(crate) fn record(&mut self, source: BodySource) -> SourceId {
        let id = SourceId {
            owner: 0,
            local: u32::try_from(self.entries.len()).expect("body occurrences fit u32"),
        };
        self.entries.push(source);
        id
    }

    /// The ID and this table come from the same prepared body. A missing entry is a broken IR
    /// relationship, rather than a source occurrence that happens to be uneditable.
    pub fn source(&self, id: SourceId) -> BodySource {
        assert_eq!(id.owner, 0, "body occurrence belongs to this source table");
        self.entries[id.local as usize]
    }

    pub fn occurrence(&self, id: SourceId) -> Option<SourceOccurrenceLocation> {
        let location = self.location(id)?;
        let copy = if location.kind == SourceLocationKind::MacroArgument {
            // Entries were appended in source ID order, independent of written offsets.
            self.argument_uses
                .binary_search_by_key(&id.local, |usage| usage.source)
                .ok()
                .map(|index| self.argument_uses[index].copy)
        } else {
            None
        };
        Some(SourceOccurrenceLocation { location, copy })
    }

    pub fn span(&self, id: SourceId) -> Span {
        self.source(id).span
    }

    /// Whether lowering proved this capture's exact range in a written template.
    pub fn is_format_capture(&self, expr: ExprId, span: Span) -> bool {
        self.format_captures
            .binary_search_by_key(&expr.0, |capture| capture.expr.0)
            .ok()
            .is_some_and(|index| self.format_captures[index].span == span)
    }

    pub(crate) fn record_format_capture(&mut self, expr: ExprId, span: Span) {
        self.format_captures
            .push(FormatCaptureSource { expr, span });
    }

    pub(crate) fn finish_recording(&mut self) {
        // Order captures once so expression scans can look them up individually.
        self.format_captures
            .sort_unstable_by_key(|capture| capture.expr.0);
    }

    /// Bounded display text for this initializer or a declaration recorded inside this body.
    pub fn initializer_preview(&self, id: SourceId) -> Option<&str> {
        self.initializer_previews.get(&id).map(String::as_str)
    }

    pub(crate) fn record_initializer_preview(&mut self, id: SourceId, preview: Option<String>) {
        if let Some(preview) = preview {
            self.initializer_previews.insert(id, preview);
        }
    }

    pub(crate) fn recorder<'a>(
        &'a mut self,
        map: &'a dyn Fn(TextRange, SourceRole) -> (BodySource, Option<MacroCopyRef>),
    ) -> BodySourceRecorder<'a> {
        BodySourceRecorder { store: self, map }
    }
}

impl SourceLocations for BodySourceStore {
    fn location(&self, id: SourceId) -> Option<SourceLocation> {
        if id.owner != 0 {
            return None;
        }
        Some(self.entries.get(id.local as usize)?.location())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
struct MacroArgumentUse {
    source: u32,
    copy: MacroCopyRef,
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
struct FormatCaptureSource {
    expr: ExprId,
    span: Span,
}

/// A lowering operation fixes its mapper before it records any ranges. Generated syntax can
/// therefore use the same declaration/type lowerers without changing the meaning of stored IDs.
pub(crate) struct BodySourceRecorder<'a> {
    store: &'a mut BodySourceStore,
    map: &'a dyn Fn(TextRange, SourceRole) -> (BodySource, Option<MacroCopyRef>),
}

impl RecordSource for BodySourceRecorder<'_> {
    fn record_as(&mut self, range: TextRange, role: SourceRole) -> SourceId {
        let (source, copy) = (self.map)(range, role);
        let id = self.store.record(source);
        if let Some(copy) = copy {
            self.store.argument_uses.push(MacroArgumentUse {
                source: id.local,
                copy,
            });
        }
        id
    }

    fn record_initializer(&mut self, initializer: &ast::Expr) -> SourceId {
        let id = self.record(initializer.syntax().text_range());
        self.store
            .record_initializer_preview(id, initializer_preview(initializer));
        id
    }
}

impl TypeSource for BodySourceRecorder<'_> {
    fn record_type(&mut self, range: TextRange) -> Option<SourceId> {
        Some(self.record(range))
    }
}
