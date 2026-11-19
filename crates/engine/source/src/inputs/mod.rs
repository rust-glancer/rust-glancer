//! Exact source and path observations shared by providers in one saved or editor input view.

mod observation;

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use rg_ir_model::FileId;
use rg_std::{MemoryRecorder, MemorySize as _, NativeOsString};

use self::observation::{FileCapture, PathObservation, SourceOrigin, SourceRoots};
use crate::{CapturedSource, QueryContext, SourceEntry, SourceError, read_source_text};

/// Source contents and path observations for a saved or editor input view.
pub struct SourceInput;

#[ajika::input(key = FileId)]
struct SourceFile {
    #[input(
        stamp = FileCapture::stamp,
        initialize = FileCapture::init,
        lifetime = FileCapture::lifetime,
        durability = FileCapture::durability
    )]
    capture: FileCapture,
}

impl SourceInput {
    /// Start a fresh input view for a workspace graph. Lazy reads and explicit saves classify
    /// their paths against the same roots, including reads made through an editor overlay.
    pub fn snapshot<'a>(
        roots: impl IntoIterator<Item = (&'a Path, ajika::Durability)>,
    ) -> ajika::Inputs {
        ajika::Inputs::with_state(SourceRoots::new(roots))
    }

    /// Resolve a path spelling in this input view, including an observed absence. File identity
    /// stays canonical even when module discovery reaches it through another spelling.
    pub fn file_at(db: &QueryContext<'_>, path: &Path) -> ajika::Result<Option<FileId>> {
        Ok(
            SourcePresence::at(NativeOsString::from_os_str(path.as_os_str()))
                .observation(db)?
                .value,
        )
    }

    /// Drop source observations while retaining the workspace configuration and its path policy.
    pub fn forget_observations(update: &mut ajika::InputUpdate) {
        update.forget_all(SourceFile::CAPTURE);
        update.forget_all(SourcePresence::OBSERVATION);
    }

    /// Install exact saved bytes under the canonical path captured with them.
    /// Saving the same bytes as an editor capture preserves its stamp but gives it persistent
    /// lifetime, allowing a later query to promote retained editor results.
    pub fn set_saved(
        update: &mut ajika::InputUpdate,
        interner: &ajika::Interner,
        source: &CapturedSource,
    ) -> ajika::Result<()> {
        let file = FileId::for_path(interner, source.path())?;
        let origin = update.state::<SourceRoots>()?.origin(source.path());
        SourceFile::at(file).set_capture(update, FileCapture::from_document(source, origin))
    }

    /// Apply a complete editor capture over the saved baseline. Unchanged documents retain their
    /// saved inputs. Each new view starts at the baseline, so typing never grows a chain of views.
    /// `saved` must be the accepted disk baseline, even if the database is serving an editor view.
    /// For example, changing the supplied set from `[a, b]` to `[b]` restores the saved `a`.
    pub fn editor_view(
        saved: &ajika::Inputs,
        interner: &ajika::Interner,
        documents: &[CapturedSource],
    ) -> ajika::Result<ajika::Inputs> {
        let mut edited = Vec::new();
        for document in documents {
            let file = FileId::for_path(interner, document.path())?;
            // Observe the saved value before overriding it. The first edit must invalidate proofs made with
            // this file's saved durability, even if its saved capture was previously lazy.
            let previous = saved.get(SourceFile::CAPTURE, &file, interner)?;
            if let Ok(entry) = previous.value {
                if entry.revision() == document.revision() {
                    continue;
                }
                // This saved capture will be hidden by editor text. Release its reloadable
                // bytes here, because end-of-operation eviction visits the effective inputs.
                entry.evict_saved_text();
            }
            edited.push((file, document));
        }
        if edited.is_empty() {
            return Ok(saved.clone());
        }

        let edited_files = edited.iter().map(|(file, _)| *file).collect::<HashSet<_>>();
        saved.overlay().edit(|update| {
            for (file, document) in edited {
                SourceFile::at(file).set_capture(
                    update,
                    FileCapture::from_document(document, SourceOrigin::Editor),
                )?;
                SourcePresence::at(NativeOsString::from_os_str(document.path().as_os_str()))
                    .set_observation(update, PathObservation::editor(file))?;
            }
            // A module can have been found through a symlink spelling. Its presence observation
            // belongs to this editor view too, although file identity remains canonical.
            for (path, presence) in saved.entries(SourcePresence::OBSERVATION) {
                if let Some(file) = presence.value
                    && edited_files.contains(&file)
                {
                    SourcePresence::at(path)
                        .set_observation(update, PathObservation::editor(file))?;
                }
            }
            Ok(())
        })
    }

    /// An include can be backed by an optional Cargo output. Capture unreadability as a value
    /// so an ordinary source read can report it while an optional include can skip it.
    pub fn is_readable(db: &QueryContext<'_>, file: FileId) -> ajika::Result<bool> {
        Ok(SourceFile::at(file).capture(db)?.value.is_ok())
    }

    pub fn entry(db: &QueryContext<'_>, file: FileId) -> ajika::Result<Arc<SourceEntry>> {
        SourceFile::at(file)
            .capture(db)?
            .value
            .map_err(ajika::Error::application)
    }

    /// Release reloadable text while keeping the revisions needed to check later disk reads.
    /// Editor captures stay resident because their bytes cannot be recovered from disk.
    pub fn evict_saved_text(inputs: &ajika::Inputs) {
        for (_, entry) in inputs.entries(SourceFile::CAPTURE) {
            if let Ok(entry) = entry.value {
                entry.evict_saved_text();
            }
        }
    }

    /// Repack source metadata only after the host has dropped its previous generation and request
    /// working sets. Preserve entry sharing between the saved and editor tables.
    /// Ajika checks that relocation preserves each key, content stamp and input policy.
    pub fn relocate<'a>(views: impl IntoIterator<Item = &'a ajika::Inputs>) -> ajika::Result<()> {
        let mut relocated = HashMap::new();
        for inputs in views {
            inputs.relocate(SourceFile::CAPTURE, |file, capture| {
                let value = capture
                    .value
                    .as_ref()
                    .map(|entry| {
                        // Keep the old allocation alive until the pass ends so its address cannot
                        // be reused while it serves as the sharing key.
                        let (_, copy) = relocated
                            .entry(Arc::as_ptr(entry))
                            .or_insert_with(|| (Arc::clone(entry), Arc::new(entry.relocated())));
                        Arc::clone(copy)
                    })
                    .map_err(Arc::clone);
                (
                    *file,
                    FileCapture {
                        value,
                        origin: capture.origin,
                    },
                )
            })?;
            inputs.relocate(SourcePresence::OBSERVATION, |path, observation| {
                (path.clone(), *observation)
            })?;
        }
        Ok(())
    }

    /// Account for source-owned allocations in addition to Ajika's input table estimate. The
    /// recorder deduplicates entries shared by saved and editor views; enumerating never captures
    /// a new file or reloads text.
    pub fn record_memory<'a>(
        views: impl IntoIterator<Item = &'a ajika::Inputs>,
        recorder: &mut MemoryRecorder,
    ) {
        let mut paths = HashSet::new();
        for inputs in views {
            for (_, entry) in inputs.entries(SourceFile::CAPTURE) {
                match entry.value {
                    Ok(entry) if recorder.visit_shared_allocation(Arc::as_ptr(&entry).cast()) => {
                        entry.record_memory_children(recorder);
                    }
                    Err(error) if recorder.visit_shared_allocation(Arc::as_ptr(&error).cast()) => {
                        error.record_memory_children(recorder);
                    }
                    _ => {}
                }
            }
            paths.extend(
                inputs
                    .entries(SourcePresence::OBSERVATION)
                    .into_iter()
                    .map(|(path, _)| path),
            );
        }
        // Input enumeration exposes effective keys, not which backing table owns each string.
        // Estimate path storage once per spelling, excluding duplicate keys in overridden tables.
        for path in paths {
            recorder.record_approximate::<NativeOsString>(path.as_encoded_bytes().len());
        }
    }

    /// Refresh affected source captures and path probes, including paths previously observed absent.
    /// Read changed captures at this host boundary so publication can compare source revisions
    /// without demanding any semantic owner. Forgetting them would hide edits to lazy includes.
    pub fn refresh_saved_paths(
        inputs: &ajika::Inputs,
        update: &mut ajika::InputUpdate,
        interner: &ajika::Interner,
        paths: &[&Path],
    ) -> ajika::Result<bool> {
        if paths.is_empty() {
            return Ok(false);
        }
        let affected = |path: &Path| paths.iter().any(|changed| path.starts_with(changed));
        for (file, capture) in inputs.entries(SourceFile::CAPTURE) {
            let changed = match capture.value {
                Ok(entry) => affected(entry.path()),
                Err(_) => affected(&file.path(interner)?),
            };
            if changed {
                let capture = FileCapture::read(file.path(interner)?, update.state()?);
                SourceFile::at(file).set_capture(update, capture)?;
            }
        }

        let mut presence_changed = false;
        for (key, presence) in inputs.entries(SourcePresence::OBSERVATION) {
            let resolved = presence.value;
            let canonical = resolved.map(|file| file.path(interner)).transpose()?;
            // A rename notification can name only the destination. Capture disappearance too,
            // so a module can select its alternate path in this candidate.
            let disappeared = canonical.as_ref().is_some_and(|path| !path.is_file());
            if disappeared && let Some(file) = resolved {
                let capture = FileCapture::read(file.path(interner)?, update.state()?);
                SourceFile::at(file).set_capture(update, capture)?;
            }
            let spelling = native_path(key.clone())?;
            if disappeared || affected(&spelling) || canonical.as_deref().is_some_and(affected) {
                // Preserve the captured answer when a content change leaves this path intact.
                let current =
                    SourcePresence::resolve(&spelling).map_err(ajika::Error::application)?;
                if canonical != current {
                    let file = current
                        .as_deref()
                        .map(|path| FileId::for_path(interner, path))
                        .transpose()?;
                    let observation = PathObservation::saved(&spelling, file, update.state()?);
                    SourcePresence::at(key).set_observation(update, observation)?;
                    presence_changed = true;
                }
            }
        }
        Ok(presence_changed)
    }

    /// Compare captured content before and after the host's input updates. Call this before
    /// demanding new declaration queries: files first discovered by those queries should not
    /// be reported as source edits.
    pub fn changed_files(
        before: &ajika::Inputs,
        after: &ajika::Inputs,
    ) -> ajika::Result<Vec<FileId>> {
        let mut changed = Vec::new();
        for (file, capture) in after.entries(SourceFile::CAPTURE) {
            let previous = before.peek(SourceFile::CAPTURE, &file);
            if previous.as_ref().map(FileCapture::stamp).transpose()? != Some(capture.stamp()?) {
                changed.push(file);
            }
        }
        Ok(changed)
    }

    /// Check captured disk contents and path resolutions before publishing a saved candidate.
    /// Call after declaration queries so their lazily discovered sources are checked too.
    pub fn validate_saved(inputs: &ajika::Inputs, interner: &ajika::Interner) -> ajika::Result<()> {
        Self::validate_observations(inputs, interner, Err)
    }

    /// Find disk observations that no longer match their captured values after an edit burst.
    pub fn stale_saved_paths(
        inputs: &ajika::Inputs,
        interner: &ajika::Interner,
    ) -> ajika::Result<Vec<PathBuf>> {
        let mut stale = BTreeSet::new();
        Self::validate_observations(inputs, interner, |error| {
            let Some(path) = error.stale_path() else {
                return Err(error);
            };
            stale.insert(path.to_path_buf());
            Ok(())
        })?;
        Ok(stale.into_iter().collect())
    }

    fn validate_observations(
        inputs: &ajika::Inputs,
        interner: &ajika::Interner,
        mut on_error: impl FnMut(SourceError) -> Result<(), SourceError>,
    ) -> ajika::Result<()> {
        for (file, capture) in inputs.entries(SourceFile::CAPTURE) {
            let result = match capture.value {
                Ok(entry) => entry.validate_saved(),
                Err(_) => {
                    let path = file.path(interner)?;
                    if read_source_text(&path).is_ok() {
                        Err(SourceError::ReadabilityChanged { path })
                    } else {
                        Ok(())
                    }
                }
            };
            if let Err(error) = result {
                on_error(error).map_err(ajika::Error::application)?;
            }
        }
        for (path, expected) in inputs.entries(SourcePresence::OBSERVATION) {
            let SourceOrigin::Saved(_) = expected.origin else {
                continue;
            };
            let path = native_path(path)?;
            let expected = expected.value.map(|file| file.path(interner)).transpose()?;
            if let Err(error) = SourcePresence::validate(&path, expected.as_deref()) {
                on_error(error).map_err(ajika::Error::application)?;
            }
        }
        Ok(())
    }
}

#[ajika::input(key = NativeOsString)]
struct SourcePresence {
    // The key is the path spelling, which can be absent or resolve through a symlink.
    // The answer names the canonical file so a retargeted symlink changes the observation.
    #[input(
        initialize = PathObservation::init,
        stamp = PathObservation::stamp,
        lifetime = PathObservation::lifetime,
        durability = PathObservation::durability
    )]
    observation: PathObservation,
}

impl SourcePresence {
    fn resolve(path: &Path) -> Result<Option<PathBuf>, SourceError> {
        if !path.is_file() {
            return Ok(None);
        }
        path.canonicalize()
            .map(Some)
            .map_err(|source| SourceError::Io {
                path: path.to_path_buf(),
                source,
            })
    }

    fn validate(path: &Path, expected: Option<&Path>) -> Result<(), SourceError> {
        let actual = Self::resolve(path)?;
        if actual.as_deref() == expected {
            return Ok(());
        }
        if actual.is_some() != expected.is_some() {
            return Err(SourceError::ExistenceChanged {
                path: path.to_path_buf(),
                expected: expected.is_some(),
                actual: actual.is_some(),
            });
        }
        Err(SourceError::PathChanged {
            path: path.to_path_buf(),
        })
    }
}

fn native_path(path: NativeOsString) -> ajika::Result<PathBuf> {
    path.into_os_string()
        .map(PathBuf::from)
        .ok_or_else(|| ajika::Error::contract("invalid native source path"))
}
