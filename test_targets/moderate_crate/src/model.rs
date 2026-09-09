use std::fmt::{Display, Formatter, Result as FmtResult};

/// Create a note with [`Self::new`].
///
/// ```rust
/// let note = Note::new(7, "hello");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub id: usize,
    pub body: String,
}

impl Note {
    pub fn new(id: usize, body: impl Into<String>) -> Self {
        Self {
            id,
            body: body.into(),
        }
    }
}

impl Display for Note {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "#{}: {}", self.id, self.body)
    }
}

#[cfg(test)]
mod tests {
    use crate::model::Note;

    #[test]
    fn displays_note() {
        let note = Note::new(7, "hello");
        assert_eq!(note.to_string(), "#7: hello");
    }
}
