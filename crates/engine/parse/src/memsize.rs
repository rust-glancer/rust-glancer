use std::mem;

use rg_memsize::{MemoryRecorder, MemorySize};

use crate::{
    file::{LineIndexState, ParsedSource},
    line_index::LineIndexStorage,
};

pub(crate) fn record_parsed_source(source: &ParsedSource, recorder: &mut MemoryRecorder) {
    match source {
        ParsedSource::SavedFile => {}
        ParsedSource::InMemory(source) => recorder.scope("in_memory", |recorder| {
            recorder.record_heap::<str>(source.len());
            recorder.record_approximate::<std::sync::Arc<str>>(mem::size_of::<usize>());
        }),
    }
}

pub(crate) fn record_line_index_state(state: &LineIndexState, recorder: &mut MemoryRecorder) {
    match state {
        LineIndexState::Resident(line_index) => recorder.scope("resident", |recorder| {
            line_index.record_memory_children(recorder);
        }),
        LineIndexState::Offloaded(line_index) => {
            if let Some(line_index) = line_index.get() {
                recorder.scope("loaded", |recorder| {
                    line_index.record_memory_children(recorder);
                });
            }
        }
    }
}

pub(crate) fn record_line_index_storage<T>(
    storage: &LineIndexStorage<T>,
    recorder: &mut MemoryRecorder,
) where
    T: MemorySize,
{
    match storage {
        LineIndexStorage::Owned(items) => items.record_memory_children(recorder),
        LineIndexStorage::Shared { .. } => {
            let items = storage.as_slice();
            recorder.scope("items", |recorder| {
                recorder.record_heap::<T>(items.len().saturating_mul(mem::size_of::<T>()));
                for item in items {
                    item.record_memory_children(recorder);
                }
            });
        }
    }
}
