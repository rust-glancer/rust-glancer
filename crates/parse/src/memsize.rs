use std::mem;

use rg_memsize::{MemoryRecorder, MemorySize};

use crate::{
    Package, PackageFileRef, ParseDb, Position, Span, Target, TargetId, TextSpan,
    file::{FileDb, FileId, ParsedFileData},
    span::{
        LineCharRange, LineColumnSpan, LineIndex, LineIndexStorage, LineInfo, LineUtf16Metrics,
    },
};

macro_rules! record_fields {
    ($recorder:expr, $owner:expr, $($field:ident),+ $(,)?) => {
        $(
            $recorder.scope(stringify!($field), |recorder| {
                $owner.$field.record_memory_children(recorder);
            });
        )+
    };
}

impl MemorySize for ParseDb {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, workspace_root, packages);
    }
}

impl MemorySize for PackageFileRef {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, package, file);
    }
}

impl MemorySize for Package {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(
            recorder,
            self,
            id,
            package_name,
            is_workspace_member,
            origin,
            files,
            targets,
        );
    }
}

impl MemorySize for FileDb {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, parsed_files, file_ids_by_path);
    }
}

impl MemorySize for ParsedFileData {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, path, line_index, syntax);
    }
}

impl MemorySize for FileId {
    fn record_memory_children(&self, _recorder: &mut MemoryRecorder) {}
}

impl MemorySize for TargetId {
    fn record_memory_children(&self, _recorder: &mut MemoryRecorder) {}
}

impl MemorySize for Target {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, id, name, kind, src_path, root_file);
    }
}

impl MemorySize for Span {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("text", |recorder| {
            self.text.record_memory_children(recorder);
        });
    }
}

impl MemorySize for TextSpan {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, start, end);
    }
}

impl MemorySize for LineColumnSpan {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, start, end);
    }
}

impl MemorySize for Position {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, line, column);
    }
}

impl MemorySize for LineIndex {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, lines, non_ascii_lines, non_ascii_ranges,);
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

impl MemorySize for LineInfo {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, start, byte_len);
    }
}

impl MemorySize for LineUtf16Metrics {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, line, utf16_len, range_start, range_len);
    }
}

impl MemorySize for LineCharRange {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, byte_start, byte_end, utf16_start, utf16_end,);
    }
}
