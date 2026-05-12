use std::mem;

use rg_memsize::{MemoryRecorder, MemorySize};

use crate::{
    Package, PackageFileRef, PackageParseSnapshot, ParseDb, Position, Span, Target, TargetId,
    TextSpan,
    file::{FileDb, FileId, LineIndexState, ParsedFileData, ParsedFilePath, ParsedFileSnapshot},
    line_index::{
        LineCharRange, LineIndex, LineIndexSnapshot, LineIndexStorage, LineInfo, LineUtf16Metrics,
    },
    span::LineColumnSpan,
};

rg_memsize::impl_memory_size_leaf!(FileId, TargetId);

rg_memsize::impl_memory_size_children! {
    ParseDb => workspace_root, packages;
    PackageFileRef => package, file;
    Package => id, package_name, is_workspace_member, origin, files, targets;
    FileDb => parsed_files, file_ids_by_path;
    ParsedFileData => path, line_index, syntax;
    ParsedFileSnapshot => path, line_index;
    PackageParseSnapshot => files, target_root_files;
    Target => id, name, kind, src_path, root_file;
    Span => text;
    TextSpan => start, end;
    LineColumnSpan => start, end;
    Position => line, column;
    LineIndex => lines, non_ascii_lines, non_ascii_ranges;
    LineIndexSnapshot => lines, non_ascii_lines, non_ascii_ranges;
    LineInfo => start, byte_len;
    LineUtf16Metrics => line, utf16_len, range_start, range_len;
    LineCharRange => byte_start, byte_end, utf16_start, utf16_end;
}

impl MemorySize for ParsedFilePath {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        self.0.record_memory_children(recorder);
    }
}

impl MemorySize for LineIndexState {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Resident(line_index) => recorder.scope("resident", |recorder| {
                line_index.record_memory_children(recorder);
            }),
            Self::Offloaded(line_index) => {
                if let Some(line_index) = line_index.get() {
                    recorder.scope("loaded", |recorder| {
                        line_index.record_memory_children(recorder);
                    });
                }
            }
        }
    }
}

impl<T> MemorySize for LineIndexStorage<T>
where
    T: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            LineIndexStorage::Owned(items) => items.record_memory_children(recorder),
            LineIndexStorage::Shared { .. } => {
                let items = self.as_slice();
                recorder.scope("items", |recorder| {
                    recorder.record_heap::<T>(items.len().saturating_mul(mem::size_of::<T>()));
                    for item in items {
                        item.record_memory_children(recorder);
                    }
                });
            }
        }
    }
}
