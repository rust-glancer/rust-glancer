//! Private scanners that translate body source spans into cursor candidates.
//!
//! Scanner implementations are organized by query shape: point lookups,
//! whole-target source scans, dot-completion receiver scans, and shared path
//! scanners.

mod cursor;
mod dot_completion_site;
mod path_completion_site;
mod paths;
mod record_field_completion_site;
mod sites;
mod source;
mod unqualified_completion_site;

pub(super) use cursor::BodyCursorScanner;
pub(super) use dot_completion_site::DotCompletionSiteScanner;
pub(super) use path_completion_site::PathCompletionSiteScanner;
pub(super) use record_field_completion_site::RecordFieldCompletionSiteScanner;
pub(super) use source::BodySourceScanner;
pub(super) use unqualified_completion_site::UnqualifiedCompletionSiteScanner;
