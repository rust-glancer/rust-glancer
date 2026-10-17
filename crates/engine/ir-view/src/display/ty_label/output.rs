//! Buffer handling for shared type text and compact replacements.
//!
//! The renderer describes Rust syntax without borrowing individual strings. This module routes
//! shared text to both forms and keeps unsuccessful writes out of the final labels.

use std::fmt;

use super::compact::CompactOutput;

/// Concrete and compact UI text for the same type.
/// For `Option<Repeat<u8>>`, the compact form can show `Option<impl Iterator<Item = u8>>`,
/// while the concrete form keeps `Option<Repeat<u8>>` for the tooltip.
/// A length budget can shorten the compact form further to `Option<…>`; concrete text keeps
/// those arguments.
///
/// Both forms use readable names and omit default arguments. These are labels for reading;
/// source insertion has its own entry point and qualification rules.
pub struct TypePresentation {
    pub concrete: String,
    pub compact: String,
}

/// The outputs needed for one branch of the type walk.
/// A replacement's original type needs only concrete text, and its projected item type needs only
/// compact text. An absent buffer also means the corresponding rendering work can be skipped.
pub(super) struct TypeOutput<'a> {
    concrete: Option<&'a mut String>,
    compact: Option<&'a mut CompactOutput>,
}

impl<'a> TypeOutput<'a> {
    pub(super) fn concrete(text: &'a mut String) -> Self {
        Self {
            concrete: Some(text),
            compact: None,
        }
    }

    /// Render both forms together, then apply the length budget to compact text. Waiting for the
    /// whole label lets shortening choose inner groups while preserving the outer type structure.
    pub(super) fn render_presentation(
        max_length: Option<usize>,
        render: impl FnOnce(&mut TypeOutput<'_>) -> anyhow::Result<bool>,
    ) -> anyhow::Result<Option<TypePresentation>> {
        let mut concrete = String::new();
        let mut compact = CompactOutput::new(max_length);
        let known = render(&mut TypeOutput {
            concrete: Some(&mut concrete),
            compact: Some(&mut compact),
        })?;
        Ok(known.then(|| TypePresentation {
            concrete,
            compact: compact.finish(),
        }))
    }

    /// Mark only text that can be replaced as a whole. Delimiters stay outside the group:
    /// `Map<Key, Value>` records `Key, Value`, so shortening leaves a balanced `Map<…>`.
    /// All requested forms are still rendered here; only compact text records a possible elision.
    pub(super) fn collapsible<T>(
        &mut self,
        render: impl FnOnce(&mut Self) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let start = self
            .compact
            .as_deref_mut()
            .and_then(CompactOutput::begin_group);
        let result = render(self);
        if let Some(compact) = &mut self.compact {
            compact.end_group(start);
        }
        result
    }

    /// Discard this type's partial text if rendering returns `false` or an error.
    /// An unavailable raw-pointer label may already have written `*const `. Remove that prefix
    /// so an enclosing type can use `_` without keeping a broken fragment.
    pub(super) fn attempt(
        &mut self,
        render: impl FnOnce(&mut Self) -> anyhow::Result<bool>,
    ) -> anyhow::Result<bool> {
        let concrete_len = self.concrete.as_ref().map_or(0, |text| text.len());
        let compact_checkpoint = self.compact.as_ref().map(|text| text.checkpoint());
        let result = render(self);
        if !matches!(result, Ok(true)) {
            if let Some(concrete) = &mut self.concrete {
                concrete.truncate(concrete_len);
            }
            if let Some(compact) = &mut self.compact
                && let Some(checkpoint) = compact_checkpoint
            {
                compact.rollback(checkpoint);
            }
        }
        result
    }

    /// Use a compact replacement when available, keeping the original type for the tooltip.
    /// `replacement` runs first, with only compact output. Returning `false` falls back to
    /// `original` for both forms. If no compact output was requested, only `original` runs.
    pub(super) fn with_compact_replacement(
        &mut self,
        original: impl FnOnce(&mut TypeOutput<'_>) -> anyhow::Result<bool>,
        replacement: impl FnOnce(&mut TypeOutput<'_>) -> anyhow::Result<bool>,
    ) -> anyhow::Result<bool> {
        if let Some(compact) = self.compact.as_deref_mut() {
            // A replacement can fail after writing some syntax. Keep that attempt separate
            // until we know it can replace the whole compact branch.
            let mut text = compact.branch();
            if replacement(&mut TypeOutput {
                concrete: None,
                compact: Some(&mut text),
            })? {
                // The compact branch is complete. Its original arguments need only concrete
                // text, so skip any simplification work whose output would be discarded.
                if let Some(concrete) = self.concrete.as_deref_mut()
                    && !original(&mut TypeOutput::concrete(concrete))?
                {
                    return Ok(false);
                }
                compact.append(text);
                return Ok(true);
            }
        }
        // Without a usable replacement, one shared walk supplies the requested forms.
        original(self)
    }

    pub(super) fn push_str(&mut self, text: &str) {
        if let Some(concrete) = &mut self.concrete {
            concrete.push_str(text);
        }
        if let Some(compact) = &mut self.compact {
            compact.push_str(text);
        }
    }

    pub(super) fn push(&mut self, ch: char) {
        if let Some(concrete) = &mut self.concrete {
            concrete.push(ch);
        }
        if let Some(compact) = &mut self.compact {
            compact.push(ch);
        }
    }

    /// Formatting happens once before the resulting pieces are appended to both buffers.
    pub(super) fn write(&mut self, value: impl fmt::Display) {
        fmt::write(self, format_args!("{value}")).expect("string writes should not fail");
    }
}

impl fmt::Write for TypeOutput<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.push_str(text);
        Ok(())
    }
}
