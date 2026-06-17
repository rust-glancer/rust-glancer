use std::{collections::BTreeMap, error::Error, fmt};

use crate::{
    ProfileDescriptor, ProfileFilter, ProfilePathError, validate_profile_key, validate_profile_path,
};

/// Static vocabulary accepted by one profiling run.
#[derive(Debug, Clone)]
pub struct ProfileRegistry {
    descriptors: Vec<ProfileDescriptor>,
    paths: BTreeMap<&'static str, usize>,
}

impl ProfileRegistry {
    pub fn new(
        descriptors: impl IntoIterator<Item = ProfileDescriptor>,
    ) -> Result<Self, ProfileRegistryError> {
        let mut registry = Self {
            descriptors: Vec::new(),
            paths: BTreeMap::new(),
        };

        for descriptor in descriptors {
            registry.push(descriptor)?;
        }

        Ok(registry)
    }

    fn push(&mut self, descriptor: ProfileDescriptor) -> Result<(), ProfileRegistryError> {
        validate_profile_path(descriptor.path()).map_err(ProfileRegistryError::InvalidPath)?;
        validate_profile_path(descriptor.scope()).map_err(ProfileRegistryError::InvalidScope)?;

        for column in descriptor.checkpoint_columns_slice() {
            validate_profile_key(column.key, column.key)
                .map_err(ProfileRegistryError::InvalidCheckpointColumn)?;
        }

        if self.paths.contains_key(descriptor.path()) {
            return Err(ProfileRegistryError::DuplicatePath {
                path: descriptor.path(),
            });
        }

        let index = self.descriptors.len();
        self.descriptors.push(descriptor);
        self.paths.insert(descriptor.path(), index);
        Ok(())
    }

    pub fn descriptors(&self) -> &[ProfileDescriptor] {
        &self.descriptors
    }

    pub fn descriptor(&self, path: &str) -> Option<&ProfileDescriptor> {
        self.paths
            .get(path)
            .and_then(|index| self.descriptors.get(*index))
    }

    pub fn validate_filter(
        &self,
        filter: &ProfileFilter,
    ) -> Result<(), ProfileFilterValidationError> {
        if filter.is_disabled() || filter.is_all() {
            return Ok(());
        }

        for selector in filter.selectors() {
            if !self
                .descriptors
                .iter()
                .any(|descriptor| descriptor.scope() == selector.as_str())
            {
                return Err(ProfileFilterValidationError::UnknownSelector {
                    selector: selector.clone(),
                });
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileRegistryError {
    InvalidPath(ProfilePathError),
    InvalidScope(ProfilePathError),
    InvalidCheckpointColumn(ProfilePathError),
    DuplicatePath { path: &'static str },
}

impl fmt::Display for ProfileRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath(error) => write!(f, "invalid profile descriptor path: {error}"),
            Self::InvalidScope(error) => write!(f, "invalid profile descriptor scope: {error}"),
            Self::InvalidCheckpointColumn(error) => {
                write!(f, "invalid profile checkpoint column: {error}")
            }
            Self::DuplicatePath { path } => write!(f, "duplicate profile path `{path}`"),
        }
    }
}

impl Error for ProfileRegistryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidPath(error)
            | Self::InvalidScope(error)
            | Self::InvalidCheckpointColumn(error) => Some(error),
            Self::DuplicatePath { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileFilterValidationError {
    UnknownSelector { selector: String },
}

impl fmt::Display for ProfileFilterValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSelector { selector } => {
                write!(f, "profile selector `{selector}` is not registered")
            }
        }
    }
}

impl Error for ProfileFilterValidationError {}
