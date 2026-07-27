//! Orchestration between the HTTP layer and the data layer.
//!
//! Route handlers parse and map; these functions decide *how* an answer is
//! assembled — which queries run concurrently, what is cached, what invalidates,
//! and how a degraded dependency is reported rather than hidden.

pub mod connectors;
pub mod pages;
pub mod prune;
pub mod prune_reaper;
pub mod prune_scan;
pub mod search;
pub mod stats;
pub mod system;
