//! The middleware stack.
//!
//! Order matters, and it is asserted in `lib.rs` where the router is built:
//! request-id and tracing outermost so every log line is correlated, then CORS,
//! then auth, then the timeout/limit/compression trio.

pub mod auth;
pub mod errors;
pub mod metrics;
pub mod request_id;
pub mod timeout;

pub use auth::require_bearer;
pub use errors::render_errors;
pub use metrics::record_request;
pub use request_id::{propagate_request_id, MakeRequestUlid};
pub use timeout::enforce_timeout;
