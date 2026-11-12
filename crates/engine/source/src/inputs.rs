//! Exact source and path observations shared by the providers in one generation.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use rg_ir_model::FileId;
use rg_std::NativeOsString;

use crate::{CapturedSource, QueryContext, SourceEntry, SourceError, read_source_text};

#[ajika::input(key = FileId)]
pub struct SourceInput {
    #[input(stamp = capture_stamp, initialize = capture_entry)]
    capture: Result<Arc<SourceEntry>, Arc<SourceError>>,
}

impl SourceInput {
    /// Install exact saved bytes under the canonical path captured with them.
    pub fn set_saved(
        update: &mut ajika::InputUpdate,
        interner: &ajika::Interner,
        source: &CapturedSource,
    ) -> ajika::Result<()> {
        let file = FileId::for_path(interner, source.path())?;
        Self::at(file).set_capture(update, Ok(Arc::new(SourceEntry::saved_captured(source))))
    }

    /// An include can be backed by an optional Cargo output. Capture unreadability as a value
    /// so an ordinary source read can report it while an optional include can skip it.
    pub fn is_readable(db: &QueryContext<'_>, file: FileId) -> ajika::Result<bool> {
        Ok(Self::at(file).capture(db)?.is_ok())
    }

    pub fn entry(db: &QueryContext<'_>, file: FileId) -> ajika::Result<Arc<SourceEntry>> {
        Self::at(file)
            .capture(db)?
            .map_err(ajika::Error::application)
    }

    pub fn captured(inputs: &ajika::Inputs, file: FileId) -> Option<Arc<SourceEntry>> {
        inputs.peek(Self::CAPTURE, &file)?.ok()
    }

    pub fn evict_saved_text(inputs: &ajika::Inputs) {
        for (_, entry) in inputs.entries(Self::CAPTURE) {
            if let Ok(entry) = entry {
                entry.evict_saved_text();
            }
        }
    }

    /// Release captures of readable files that no target names anymore. Unreadable captures
    /// may still explain why an optional include was skipped, so retain those observations.
    pub fn retire_detached_sources(
        database: &mut crate::QueryDatabase,
        retained: impl Fn(FileId) -> bool,
    ) -> ajika::Result<()> {
        let detached = database
            .inputs()
            .entries(Self::CAPTURE)
            .into_iter()
            .filter_map(|(file, entry)| (entry.is_ok() && !retained(file)).then_some(file))
            .collect::<Vec<_>>();
        if !detached.is_empty() {
            database.update_inputs(|update| {
                for file in detached {
                    Self::at(file).forget_all(update);
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    /// Refresh affected source captures and path probes, including paths previously observed absent.
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
        for (file, capture) in inputs.entries(Self::CAPTURE) {
            let changed = match capture {
                Ok(entry) => affected(entry.path()),
                Err(_) => affected(&file.path(interner)?),
            };
            if changed {
                Self::at(file).forget_all(update);
            }
        }

        let mut presence_changed = false;
        for (key, resolved) in inputs.entries(SourcePresence::RESOLVED_FILE) {
            let canonical = resolved.map(|file| file.path(interner)).transpose()?;
            // A rename notification can name only the destination. Retire a known source
            // that disappeared so a module can select its alternate path in this candidate.
            let disappeared = canonical.as_ref().is_some_and(|path| !path.is_file());
            if disappeared && let Some(file) = resolved {
                Self::at(file).forget_all(update);
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
                    SourcePresence::at(key).set_resolved_file(update, file)?;
                    presence_changed = true;
                }
            }
        }
        Ok(presence_changed)
    }

    /// Include observations initialized during replay when validating the candidate.
    pub fn validate_saved(inputs: &ajika::Inputs, interner: &ajika::Interner) -> ajika::Result<()> {
        Self::validate_observations(inputs, interner, Err)
    }

    /// Find settled changes together after an edit burst, including newly present modules.
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
        for (file, capture) in inputs.entries(Self::CAPTURE) {
            let result = match capture {
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
        for (path, expected) in inputs.entries(SourcePresence::RESOLVED_FILE) {
            let path = native_path(path)?;
            let expected = expected.map(|file| file.path(interner)).transpose()?;
            if let Err(error) = SourcePresence::validate(&path, expected.as_deref()) {
                on_error(error).map_err(ajika::Error::application)?;
            }
        }
        Ok(())
    }
}

#[ajika::input(key = NativeOsString)]
pub struct SourcePresence {
    // The key is the path spelling, which can be absent or resolve through a symlink.
    // The answer names the canonical file so a retargeted symlink changes the observation.
    #[input(initialize = capture_path)]
    resolved_file: Option<FileId>,
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

fn capture_entry(
    file: &FileId,
    cx: &dyn ajika::InputInit,
) -> ajika::Result<Result<Arc<SourceEntry>, Arc<SourceError>>> {
    let path = file.path(&cx.interner())?;
    Ok(read_source_text(&path)
        .map(|text| Arc::new(SourceEntry::saved(path, text)))
        .map_err(Arc::new))
}

fn capture_path(path: &NativeOsString, cx: &dyn ajika::InputInit) -> ajika::Result<Option<FileId>> {
    SourcePresence::resolve(&native_path(path.clone())?)
        .map_err(ajika::Error::application)?
        .as_deref()
        .map(|path| FileId::for_path(&cx.interner(), path))
        .transpose()
}

fn native_path(path: NativeOsString) -> ajika::Result<PathBuf> {
    path.into_os_string()
        .map(PathBuf::from)
        .ok_or_else(|| ajika::Error::contract("invalid native source path"))
}

fn capture_stamp(
    capture: &Result<Arc<SourceEntry>, Arc<SourceError>>,
) -> ajika::Result<ajika::Stamp> {
    // Evicting saved text doesn't change what a reader can observe. SourceEntry verifies the
    // revision if it has to reload those bytes from disk.
    Ok(match capture {
        Ok(entry) => ajika::Stamp::from_bytes(*entry.revision().as_bytes()),
        Err(_) => ajika::Stamp::of(b"unreadable Rust source"),
    })
}
