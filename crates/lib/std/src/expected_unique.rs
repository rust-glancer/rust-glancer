use crate::{MemoryRecorder, MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Candidate accumulator for values that are expected to be unique.
///
/// `One` is the usable result. `Empty` and `Ambiguous` are both invalid for consumers that need a
/// single value, but keeping them separate helps lookup code preserve intent until the boundary.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub enum ExpectedUnique<T> {
    Empty,
    One(T),
    Ambiguous,
}

impl<T> MemorySize for ExpectedUnique<T>
where
    T: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        if let Self::One(value) = self {
            value.record_memory_children(recorder);
        }
    }
}

impl<T> Shrink for ExpectedUnique<T>
where
    T: Shrink,
{
    fn shrink_to_fit(&mut self) {
        if let Self::One(value) = self {
            value.shrink_to_fit();
        }
    }
}

impl<T> Default for ExpectedUnique<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ExpectedUnique<T> {
    pub fn new() -> Self {
        Self::Empty
    }

    pub fn as_ref(&self) -> ExpectedUnique<&T> {
        match self {
            Self::Empty => ExpectedUnique::Empty,
            Self::One(value) => ExpectedUnique::One(value),
            Self::Ambiguous => ExpectedUnique::Ambiguous,
        }
    }

    pub fn as_option(&self) -> Option<&T> {
        match self {
            Self::One(value) => Some(value),
            Self::Empty | Self::Ambiguous => None,
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    pub fn is_one(&self) -> bool {
        matches!(self, Self::One(_))
    }

    pub fn is_ambiguous(&self) -> bool {
        matches!(self, Self::Ambiguous)
    }

    pub fn into_option(self) -> Option<T> {
        match self {
            Self::One(value) => Some(value),
            Self::Empty | Self::Ambiguous => None,
        }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> ExpectedUnique<U> {
        match self {
            Self::Empty => ExpectedUnique::Empty,
            Self::One(value) => ExpectedUnique::One(f(value)),
            Self::Ambiguous => ExpectedUnique::Ambiguous,
        }
    }
}

impl<T> ExpectedUnique<T>
where
    T: PartialEq,
{
    /// Returns whether the usable value equals `value`.
    pub fn is(&self, value: &T) -> bool {
        matches!(self, Self::One(existing) if existing == value)
    }

    /// Adds a value if empty, does nothing if the same value is already stored,
    /// and makes the value ambiguous otherwise.
    pub fn push(&mut self, value: T) {
        match self {
            Self::Empty => *self = Self::One(value),
            Self::One(existing) if *existing == value => {}
            Self::One(_) | Self::Ambiguous => *self = Self::Ambiguous,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ExpectedUnique;

    #[test]
    fn tracks_empty_unique_and_ambiguous_states() {
        let mut candidate = ExpectedUnique::new();
        assert_eq!(candidate, ExpectedUnique::Empty);

        candidate.push("A");
        assert_eq!(candidate, ExpectedUnique::One("A"));

        candidate.push("A");
        assert_eq!(candidate, ExpectedUnique::One("A"));

        candidate.push("B");
        assert_eq!(candidate, ExpectedUnique::Ambiguous);

        candidate.push("B");
        assert_eq!(candidate, ExpectedUnique::Ambiguous);
    }

    #[test]
    fn is_only_reports_one_matching_value() {
        assert!(ExpectedUnique::One("A").is(&"A"));
        assert!(!ExpectedUnique::One("A").is(&"B"));
        assert!(!ExpectedUnique::<&str>::Empty.is(&"A"));
        assert!(!ExpectedUnique::<&str>::Ambiguous.is(&"A"));
    }

    #[test]
    fn exposes_only_one_as_option() {
        assert_eq!(ExpectedUnique::<u8>::new().as_option(), None);
        assert_eq!(ExpectedUnique::One(1).as_option(), Some(&1));
        assert_eq!(ExpectedUnique::<u8>::Ambiguous.as_option(), None);

        assert_eq!(ExpectedUnique::<u8>::new().into_option(), None);
        assert_eq!(ExpectedUnique::One(1).into_option(), Some(1));
        assert_eq!(ExpectedUnique::<u8>::Ambiguous.into_option(), None);
    }
}
