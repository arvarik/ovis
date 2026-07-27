//! The OpenSearch chunk index and the embedding endpoint.
//!
//! OVIS asks OpenSearch exactly one kind of question: *what is in this content?*
//! Counting, filtering, sorting and listing are Postgres's job. The list path
//! makes no calls into this module at all.

pub mod embed;
pub mod os_client;
pub mod query;

pub use embed::EmbedClient;
pub use os_client::{IndexCapabilities, OsClient, RawSearchHit, RawSearchResults};
pub use query::{SearchFilters, SearchRequest};
