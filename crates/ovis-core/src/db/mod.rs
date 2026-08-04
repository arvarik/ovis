//! SQL against the Onyx Postgres database.
//!
//! Onyx owns this schema; OVIS treats it as an external, version-drift-prone
//! dependency. That has three consequences enforced here:
//!
//! 1. Queries are runtime-checked (`sqlx::query*`, not the `query!` macros),
//!    because a compile-time macro would need a live Onyx database at build
//!    time.
//! 2. [`probe`] verifies at startup that every column we read still exists, and
//!    which foreign keys point at `document(id)`. A drift becomes a loud
//!    `501 SCHEMA_MISMATCH` on the affected endpoint, never a wrong answer.
//! 3. The only tables OVIS writes are `document` and its FK children (for
//!    per-document delete/edit, which the Onyx API does not expose) and its own
//!    `ovis` schema. Every connector/indexing action goes through the Onyx API.

pub mod connectors;
pub mod documents;
pub mod indexing;
pub mod pending_deletes;
pub mod pool;
pub mod probe;
pub mod profile;
pub mod prune;
pub mod stats;
pub mod tags;
pub mod trash;

pub use pool::create_pg_pool;
