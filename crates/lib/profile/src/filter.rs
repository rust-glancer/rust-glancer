use std::{error::Error, fmt};

use crate::{ProfilePathError, validate_profile_path};

/// Controls which registered profile scopes are collected during a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileFilter {
    mode: ProfileFilterMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProfileFilterMode {
    Disabled,
    All,
    Selectors(Vec<String>),
}

impl ProfileFilter {
    pub fn disabled() -> Self {
        Self {
            mode: ProfileFilterMode::Disabled,
        }
    }

    pub fn all() -> Self {
        Self {
            mode: ProfileFilterMode::All,
        }
    }

    pub fn parse(text: &str) -> Result<Self, ProfileFilterParseError> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(Self::disabled());
        }

        if text == "all" || text == "*" {
            return Ok(Self::all());
        }

        let mut selectors = Vec::new();
        for selector in text.split(',') {
            let selector = selector.trim();
            validate_profile_path(selector).map_err(ProfileFilterParseError::InvalidSelector)?;
            selectors.push(selector.to_string());
        }

        Ok(Self {
            mode: ProfileFilterMode::Selectors(selectors),
        })
    }

    pub fn selectors(&self) -> &[String] {
        match &self.mode {
            ProfileFilterMode::Selectors(selectors) => selectors,
            ProfileFilterMode::Disabled | ProfileFilterMode::All => &[],
        }
    }

    pub fn is_disabled(&self) -> bool {
        matches!(self.mode, ProfileFilterMode::Disabled)
    }

    pub fn is_all(&self) -> bool {
        matches!(self.mode, ProfileFilterMode::All)
    }

    pub(crate) fn enables_scope(&self, scope: &str) -> bool {
        match &self.mode {
            ProfileFilterMode::Disabled => false,
            ProfileFilterMode::All => true,
            ProfileFilterMode::Selectors(selectors) => selectors
                .iter()
                .any(|selector| path_is_ancestor_or_equal(scope, selector)),
        }
    }
}

fn path_is_ancestor_or_equal(ancestor: &str, descendant: &str) -> bool {
    ancestor == descendant
        || descendant
            .strip_prefix(ancestor)
            .is_some_and(|suffix| suffix.starts_with('.'))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileFilterParseError {
    InvalidSelector(ProfilePathError),
}

impl fmt::Display for ProfileFilterParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSelector(error) => error.fmt(f),
        }
    }
}

impl Error for ProfileFilterParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidSelector(error) => Some(error),
        }
    }
}
