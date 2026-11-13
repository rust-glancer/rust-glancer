//! Physical occurrences for one body, resolved while its syntax and macro maps are available.

use rg_ir_model::{BodySource, SourceId, SourceLocation, Span};
use rg_item_tree::{RecordSource, SourceLocations, SourceRole, TypeSource};
use rg_std::{MemorySize, Shrink};
use rg_syntax::TextRange;
use wincode::{SchemaRead, SchemaWrite};

use super::BodySourceMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub(crate) struct BodyLocation {
    pub(crate) source: BodySource,
    // A copied macro argument is still generated syntax, but its original bytes can be edited.
    // This is separate from BodySource's direct-written/generated distinction.
    pub(crate) editable: bool,
}

/// Source data for a single body's occurrence IDs. Nested bodies have their own store.
///
/// All occurrences in this store use owner zero. Unlike a file's declaration table, a body is
/// prepared and compared as a whole, so its expressions and local type syntax can share one
/// recording sequence. Comments and changing byte offsets do not change that sequence.
#[derive(Debug, Default, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodySourceStore {
    entries: Vec<BodyLocation>,
    pub(crate) provenance: BodySourceMap,
}

impl BodySourceStore {
    pub(crate) fn record(&mut self, source: BodySource) -> SourceId {
        self.record_location(BodyLocation {
            source,
            editable: source.is_written(),
        })
    }

    fn record_location(&mut self, location: BodyLocation) -> SourceId {
        let id = SourceId {
            owner: 0,
            local: u32::try_from(self.entries.len()).expect("body occurrences fit u32"),
        };
        self.entries.push(location);
        id
    }

    /// The ID and this table come from the same prepared body. A missing entry is a broken IR
    /// relationship, rather than a source occurrence that happens to be uneditable.
    pub fn source(&self, id: SourceId) -> BodySource {
        assert_eq!(id.owner, 0, "body occurrence belongs to this source table");
        self.entries[id.local as usize].source
    }

    pub fn span(&self, id: SourceId) -> Span {
        self.source(id).span
    }

    pub fn provenance(&self) -> &BodySourceMap {
        &self.provenance
    }

    pub(crate) fn recorder<'a>(
        &'a mut self,
        map: &'a dyn Fn(TextRange, SourceRole) -> BodyLocation,
    ) -> BodySourceRecorder<'a> {
        BodySourceRecorder { store: self, map }
    }
}

impl SourceLocations for BodySourceStore {
    fn location(&self, id: SourceId) -> Option<SourceLocation> {
        if id.owner != 0 {
            return None;
        }
        let entry = self.entries.get(id.local as usize)?;
        Some(SourceLocation {
            span: entry.source.span,
            editable: entry.editable,
        })
    }
}

/// A lowering operation fixes its mapper before it records any ranges. Generated syntax can
/// therefore use the same declaration/type lowerers without changing the meaning of stored IDs.
pub(crate) struct BodySourceRecorder<'a> {
    store: &'a mut BodySourceStore,
    map: &'a dyn Fn(TextRange, SourceRole) -> BodyLocation,
}

impl RecordSource for BodySourceRecorder<'_> {
    fn record_as(&mut self, range: TextRange, role: SourceRole) -> SourceId {
        self.store.record_location((self.map)(range, role))
    }
}

impl TypeSource for BodySourceRecorder<'_> {
    fn record_type(&mut self, range: TextRange) -> Option<SourceId> {
        Some(self.record(range))
    }
}
