//! Names and Rust editions shared by parsing, analysis, and editor features.
//!
//! A name such as `User` appears in declarations, types, and references to that declaration.
//! Those parts of the engine need to agree on its spelling when comparing or looking up names.
//! This crate gives them [`Name`] for that text and [`RustEdition`] for the edition rules used
//! to read source.

use std::{
    borrow::{Borrow, Cow},
    fmt,
    ops::Deref,
};

use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Which edition's rules apply to a Rust source file.
///
/// Parsing and name lookup both need this. For example, the edition determines which standard
/// prelude is available in the file.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, SchemaRead, SchemaWrite, MemorySize, Shrink,
)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum RustEdition {
    Edition2015,
    Edition2018,
    Edition2021,
    #[default]
    Edition2024,
}

impl RustEdition {
    /// The module to look up inside `std::prelude` or `core::prelude`.
    /// For example, Rust 2021 uses `std::prelude::rust_2021`.
    pub fn prelude_module(self) -> &'static str {
        match self {
            Self::Edition2015 => "rust_2015",
            Self::Edition2018 => "rust_2018",
            Self::Edition2021 => "rust_2021",
            Self::Edition2024 => "rust_2024",
        }
    }
}

impl fmt::Display for RustEdition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Edition2015 => "2015",
            Self::Edition2018 => "2018",
            Self::Edition2021 => "2021",
            Self::Edition2024 => "2024",
        })
    }
}

/// Text used to look up an identifier, lifetime, or label.
///
/// A `Name` uses interned strings via `string_cache`.
///
/// Rust's `r#` prefix affects parsing, but not which name is looked up. We remove it when creating
/// a `Name`, including after the apostrophe in a lifetime or label:
///
/// ```
/// use rg_text::Name;
///
/// assert_eq!(Name::new("r#type"), Name::new("type"));
/// assert_eq!(Name::new("'r#fn").as_str(), "'fn");
/// ```
///
/// Keep an ordinary string when you need the spelling exactly as written in source.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Name(string_cache::DefaultAtom);

impl Name {
    pub fn new(text: impl AsRef<str>) -> Self {
        Self(Self::canonical_text(Cow::Borrowed(text.as_ref())).into())
    }

    /// Keep lowering incomplete source by using `<missing>` where a name is required.
    pub fn missing() -> Self {
        Self::new("<missing>")
    }

    pub fn is_missing(&self) -> bool {
        self.as_str() == "<missing>"
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Constructors and cache decoding remove `r#` here so they store the same spelling.
    fn canonical_text(text: Cow<'_, str>) -> Cow<'_, str> {
        let start = if text.starts_with("r#") {
            0
        } else if text.starts_with("'r#") {
            1
        } else {
            return text;
        };
        // For `r#type`, we can borrow the suffix `type`. For `'r#fn`, we need to keep the
        // apostrophe and remove the middle `r#`, which requires editing an owned string.
        match text {
            Cow::Borrowed(text) if start == 0 => Cow::Borrowed(&text[2..]),
            text => {
                let mut text = text.into_owned();
                text.drain(start..start + 2);
                Cow::Owned(text)
            }
        }
    }
}

impl Shrink for Name {
    fn shrink_to_fit(&mut self) {}
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for Name {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl Deref for Name {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl From<&str> for Name {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for Name {
    fn from(value: String) -> Self {
        Self(Self::canonical_text(Cow::Owned(value)).into())
    }
}

// A map keyed by Name can be searched with a string, for example `map.get("User")`.
// The Name and the lookup string must produce the same hash for that lookup to work.
// Use the string's hash rather than the atom's cached hash.
impl std::hash::Hash for Name {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

// Write the name's text to the cache. The pool's handles only make sense in this process;
// a later run needs the spelling so it can create its own Name from it.
unsafe impl<C> SchemaWrite<C> for Name
where
    C: wincode::config::Config,
{
    type Src = Name;

    fn size_of(src: &Self::Src) -> wincode::WriteResult<usize> {
        <str as SchemaWrite<C>>::size_of(src.as_str())
    }

    fn write(writer: impl wincode::io::Writer, src: &Self::Src) -> wincode::WriteResult<()> {
        <str as SchemaWrite<C>>::write(writer, src.as_str())
    }
}

unsafe impl<'de, C> SchemaRead<'de, C> for Name
where
    C: wincode::config::Config,
{
    type Dst = Name;

    fn read(
        reader: impl wincode::io::Reader<'de>,
        dst: &mut std::mem::MaybeUninit<Self::Dst>,
    ) -> wincode::ReadResult<()> {
        // Cow lets a slice reader lend the text without allocating a temporary String.
        // A stream reader supplies a String instead. The string pool accepts either form
        // and makes the resulting Name independent of the serialized buffer.
        let text = <Cow<'de, str> as SchemaRead<C>>::get(reader)?;
        dst.write(Self(Self::canonical_text(text).into()));
        Ok(())
    }
}

impl PartialEq<str> for Name {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for Name {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl MemorySize for Name {
    fn record_memory_children(&self, recorder: &mut rg_std::MemoryRecorder) {
        // Many items can hold the same name and share its string allocation. Count that
        // allocation only on the first visit. Names stored inside the value or in static
        // data have no heap allocation to count here.
        if self.0.is_dynamic()
            && recorder.visit_shared_allocation(self.as_str().as_ptr().cast::<()>())
        {
            recorder.record_heap::<str>(self.len());
            // A pool entry also stores the string's pointer and length, a reference count,
            // a next-entry pointer, and a u64 hash. Estimate that entry's size separately.
            recorder.record_approximate::<Name>(std::mem::size_of::<usize>() * 4 + 8);
        }
        // The pool's shared lookup table stays alive for the whole process. Leave it out of
        // per-value reports; its cost and allocator overhead show up in process measurements.
    }
}

/// Remove `r#` when comparing or looking up an identifier directly from syntax.
///
/// For example, module discovery reads `mod r#type;` as the module name `type`.
/// This helper avoids creating a `Name` when the caller only needs the spelling for a lookup.
pub fn identifier_text(text: &str) -> &str {
    text.strip_prefix("r#").unwrap_or(text)
}

#[cfg(test)]
mod tests;
