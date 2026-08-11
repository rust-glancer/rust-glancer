//! Cross-family completion coverage for the source states an editor sends while a user types.
//!
//! The cases deliberately separate forward typing without a terminator, an empty prefix directly
//! after trigger punctuation, and editing inside syntax that already has its closing tokens. This
//! keeps provider tests from passing only because an otherwise-complete fixture helps the parser.

mod candidates;
mod contexts;
mod semantic;
mod specialized;
mod standalone;
