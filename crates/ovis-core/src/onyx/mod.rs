//! The Onyx HTTP API.
//!
//! Every state-changing operation on a connector goes through here rather than
//! through direct SQL. Onyx owns the surrounding machinery — Celery task
//! signalling, index bookkeeping, permission sync — and writing its tables
//! behind its back would leave that machinery out of step.
//!
//! The one exception is per-document delete and edit, which Onyx exposes no
//! endpoint for; those live in [`crate::db::documents`].

pub mod client;

pub use client::{OnyxAuth, OnyxClient, OnyxVersion, PatCredentials};
