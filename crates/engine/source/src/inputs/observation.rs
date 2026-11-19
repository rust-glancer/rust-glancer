//! Source input values keep their content and cache policy separate. Saving editor text can
//! change its lifetime and durability even when its content stamp stays the same.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use ajika::{Durability, InputInit, Lifetime, Stamp};
use rg_ir_model::FileId;
use rg_std::NativeOsString;

use super::{SourcePresence, native_path};
use crate::{CapturedSource, SourceEntry, SourceError, read_source_text};

pub(super) struct SourceRoots(HashMap<PathBuf, Durability>);

impl SourceRoots {
    pub(super) fn new<'a>(roots: impl IntoIterator<Item = (&'a Path, Durability)>) -> Self {
        let mut classes = HashMap::<PathBuf, Durability>::new();
        for (path, durability) in roots {
            // Opening an installed crate as a workspace member makes that same root editable.
            let entry = classes.entry(path.to_owned()).or_insert(durability);
            *entry = (*entry).min(durability);
        }
        Self(classes)
    }

    pub(super) fn origin(&self, path: &Path) -> SourceOrigin {
        // The nearest root wins for nested packages. Unknown paths, including arbitrary
        // include! targets, should be treated like ordinary editable source.
        let durability = path
            .ancestors()
            .find_map(|root| self.0.get(root).copied())
            .unwrap_or(Durability::Medium);
        SourceOrigin::Saved(durability)
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum SourceOrigin {
    Saved(Durability),
    Editor,
}

#[derive(Clone, Copy)]
pub(super) struct Observation<T> {
    pub(super) value: T,
    pub(super) origin: SourceOrigin,
}

impl<T> Observation<T> {
    pub(super) fn lifetime(&self) -> Lifetime {
        match self.origin {
            SourceOrigin::Saved(_) => Lifetime::Persistent,
            SourceOrigin::Editor => Lifetime::Session,
        }
    }

    pub(super) fn durability(&self) -> Durability {
        match self.origin {
            SourceOrigin::Saved(durability) => durability,
            SourceOrigin::Editor => Durability::Low,
        }
    }
}

pub(super) type FileCapture = Observation<Result<Arc<SourceEntry>, Arc<SourceError>>>;

impl FileCapture {
    pub(super) fn from_document(source: &CapturedSource, origin: SourceOrigin) -> Self {
        let entry = match origin {
            SourceOrigin::Saved(_) => SourceEntry::saved_captured(source),
            SourceOrigin::Editor => SourceEntry::editor(source),
        };
        Self {
            value: Ok(Arc::new(entry)),
            origin,
        }
    }

    pub(super) fn init(file: &FileId, cx: &dyn InputInit) -> ajika::Result<Self> {
        Ok(Self::read(file.path(&cx.interner())?, cx.state()?))
    }

    pub(super) fn read(path: PathBuf, roots: &SourceRoots) -> Self {
        // Optional includes can probe readability. Keep failed reads under the same saved
        // policy as successful reads from this path.
        let origin = roots.origin(&path);
        let value = read_source_text(&path)
            .map(|text| Arc::new(SourceEntry::saved(path, text)))
            .map_err(Arc::new);
        Self { value, origin }
    }

    pub(super) fn stamp(&self) -> ajika::Result<Stamp> {
        Ok(match &self.value {
            Ok(entry) => entry.revision().stamp(),
            // A readable source is UTF-8, so its bytes can never equal this error marker.
            Err(_) => Stamp::of(&[0xff]),
        })
    }
}

pub(super) type PathObservation = Observation<Option<FileId>>;

impl PathObservation {
    pub(super) fn editor(file: FileId) -> Self {
        Self {
            value: Some(file),
            origin: SourceOrigin::Editor,
        }
    }

    pub(super) fn saved(spelling: &Path, file: Option<FileId>, roots: &SourceRoots) -> Self {
        // The key names the path we probed, even when it is absent or resolves via a symlink.
        Self {
            value: file,
            origin: roots.origin(spelling),
        }
    }

    pub(super) fn init(path: &NativeOsString, cx: &dyn InputInit) -> ajika::Result<Self> {
        let spelling = native_path(path.clone())?;
        let value = SourcePresence::resolve(&spelling)
            .map_err(ajika::Error::application)?
            .as_deref()
            .map(|path| FileId::for_path(&cx.interner(), path))
            .transpose()?;
        Ok(Self::saved(&spelling, value, cx.state()?))
    }

    pub(super) fn stamp(&self) -> ajika::Result<Stamp> {
        Stamp::of_value(&self.value)
    }
}
