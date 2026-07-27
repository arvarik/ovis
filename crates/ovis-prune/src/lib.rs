//! The pruning detection engine: configuration and the pure detectors.
//!
//! This crate holds everything about *finding* prune candidates that does not
//! need a database: MinHash/LSH near-duplicate detection, and the detector
//! configuration with its YAML round-trip. The backend feeds it data from the
//! API-era data layer and owns all persistence and lifecycle.
//!
//! The 2026 rework kept the tested algorithmic core (`dedup`) and the config
//! story, and replaced the old self-contained engine/reporter — they predated
//! the API-first architecture and pulled their own data.

pub mod config;
pub mod content;
pub mod dedup;

pub use config::*;
pub use content::*;
pub use dedup::*;
