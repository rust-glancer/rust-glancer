use rg_std::{Cancelled, OperationError};

/// Semantic reads may stop because their query failed or their request was cancelled.
#[derive(Debug)]
pub enum ReadError {
    Query(ajika::Error),
    Cancelled(Cancelled),
}

impl ReadError {
    /// Recover a query failure or a host cancellation from a semantic error report.
    /// Wrapping either one as an application error would hide its kind from Ajika.
    pub fn query_cause(error: &(dyn std::error::Error + 'static)) -> Option<ajika::Error> {
        std::iter::successors(Some(error), |&cause| cause.source()).find_map(|cause| {
            if let Some(error) = cause.downcast_ref::<ajika::Error>() {
                return Some(error.clone());
            }
            cause
                .downcast_ref::<Cancelled>()
                .map(|&cancelled| Self::Cancelled(cancelled).into())
        })
    }
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Query(error) => error.fmt(f),
            Self::Cancelled(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::Query(error) => error,
            Self::Cancelled(error) => error,
        })
    }
}

impl From<OperationError<Self>> for ReadError {
    fn from(error: OperationError<Self>) -> Self {
        match error {
            OperationError::Source(error) => error,
            OperationError::Cancelled(cancelled) => Self::Cancelled(cancelled),
        }
    }
}

impl From<ReadError> for ajika::Error {
    fn from(error: ReadError) -> Self {
        match error {
            ReadError::Query(error) => error,
            ReadError::Cancelled(cancelled) => ajika::Cancelled::at(cancelled.checkpoint()).into(),
        }
    }
}
