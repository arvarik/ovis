//! Errors and exit codes.
//!
//! Two rules the old CLI broke:
//!
//! 1. **A failure is a failure.** Every command path ended in `Ok(())` and exit
//!    0, including the ones that fell back to baked-in sample data. Here a
//!    failure is an [`CliError`] with a non-zero exit code, and there is no
//!    fallback data to fall back to.
//! 2. **The exit code says which kind of failure it was** (the table lives in
//!    `docs/cli.md`), so a script can branch without parsing English.

use std::fmt;

/// Exit codes. The table lives in `01_COMMAND_TREE.md` §6; this is the code that
/// implements it.
pub mod exit {
    pub const OK: i32 = 0;
    pub const GENERIC: i32 = 1;
    pub const USAGE: i32 = 2;
    pub const NOT_FOUND: i32 = 3;
    pub const NEEDS_CONFIRMATION: i32 = 10;
    pub const PARTIAL_FAILURE: i32 = 11;
    pub const UNREACHABLE: i32 = 12;
    pub const DEGRADED: i32 = 13;
    pub const STALE_HANDLE: i32 = 14;
}

/// The server's error envelope, as specified in `backend/03_API_SURFACE.md` and
/// emitted by `ovis_backend::error::ErrorEnvelope`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ApiErrorBody {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub status: u16,
    #[serde(default)]
    pub req_id: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ApiErrorEnvelope {
    pub error: ApiErrorBody,
}

#[derive(Debug)]
pub enum CliError {
    /// A non-2xx response carrying the documented envelope.
    Api(ApiErrorBody),
    /// A non-2xx response that was not the documented envelope (a proxy error
    /// page, an HTML SPA fallback, …). Kept distinct so the message can say the
    /// server answered but not in the expected shape.
    Http { status: u16, body: String },
    /// Could not reach the server at all.
    Unreachable { url: String, detail: String },
    /// Confirmation was required and `--no-input` (or a missing terminal)
    /// prevented asking.
    NeedsConfirmation(String),
    /// A batch operation partly failed. The message already lists the failures.
    PartialFailure(String),
    /// `ovis status` found the server degraded.
    Degraded(String),
    /// An `@N` handle could not be resolved.
    StaleHandle(String),
    /// A usage mistake clap could not catch (mutually exclusive flags resolved
    /// at runtime, an unparsable `--since`, …).
    Usage(String),
    /// Anything else.
    Other(anyhow::Error),
}

impl CliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::Api(body) => match body.code.as_str() {
                "NOT_FOUND" => exit::NOT_FOUND,
                "BAD_REQUEST" => exit::USAGE,
                _ => exit::GENERIC,
            },
            CliError::Http { .. } => exit::GENERIC,
            CliError::Unreachable { .. } => exit::UNREACHABLE,
            CliError::NeedsConfirmation(_) => exit::NEEDS_CONFIRMATION,
            CliError::PartialFailure(_) => exit::PARTIAL_FAILURE,
            CliError::Degraded(_) => exit::DEGRADED,
            CliError::StaleHandle(_) => exit::STALE_HANDLE,
            CliError::Usage(_) => exit::USAGE,
            CliError::Other(_) => exit::GENERIC,
        }
    }

    /// The `error:` line.
    pub fn message(&self) -> String {
        match self {
            CliError::Api(body) => {
                let mut msg = body.message.clone();
                if !body.code.is_empty() {
                    msg.push_str(&format!(" (code {}", body.code));
                    if !body.req_id.is_empty() && body.req_id != "-" {
                        msg.push_str(&format!(", req {}", body.req_id));
                    }
                    msg.push(')');
                }
                msg
            }
            CliError::Http { status, body } => {
                // One line, not a wall: an HTML error page is 40 lines of head
                // and the useful part is the status code.
                let excerpt: String = body
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(160)
                    .collect();
                if excerpt.trim().is_empty() {
                    format!("the server answered HTTP {status} with an empty body")
                } else {
                    format!("unexpected HTTP {status} response: {excerpt}")
                }
            }
            CliError::Unreachable { url, detail } => {
                format!("cannot reach OVIS server at {url}: {detail}")
            }
            CliError::NeedsConfirmation(what) => what.clone(),
            CliError::PartialFailure(what) => what.clone(),
            CliError::Degraded(what) => what.clone(),
            CliError::StaleHandle(what) => what.clone(),
            CliError::Usage(what) => what.clone(),
            CliError::Other(err) => format!("{err:#}"),
        }
    }

    /// The optional `hint:` line — what to actually do about it.
    pub fn hint(&self) -> Option<String> {
        match self {
            CliError::Unreachable { .. } => Some(
                "start it with `ovis server start -d`, or point elsewhere with \
                 --server/OVIS_SERVER"
                    .into(),
            ),
            CliError::Api(body) => match body.code.as_str() {
                "UNAUTHORIZED" => {
                    Some("the server has auth on; pass --token or set OVIS_TOKEN".into())
                }
                "ONYX_UNCONFIGURED" => Some(
                    "connector actions need an Onyx token on the server; mint one with \
                     `ovis server setup-onyx-key`"
                        .into(),
                ),
                "PARKED_CONNECTOR" => Some(
                    "this cc-pair carries a resilience-cron park sentinel; re-run with \
                     --acknowledge-parked once you are sure"
                        .into(),
                ),
                "DATABASE" | "OPENSEARCH_UPSTREAM" | "ONYX_UPSTREAM" => Some(format!(
                    "the server logged the cause under req_id {}",
                    if body.req_id.is_empty() {
                        "-"
                    } else {
                        &body.req_id
                    }
                )),
                _ => None,
            },
            // `-y` deliberately does not satisfy a name echo, so suggesting it
            // there would send the user round in a circle.
            CliError::NeedsConfirmation(msg) if msg.contains("--confirm-name") => Some(
                "supply --confirm-name with the exact name, or run without --no-input and type \
                 it when asked"
                    .into(),
            ),
            CliError::NeedsConfirmation(_) => {
                Some("pass -y to confirm, or drop --no-input and answer the prompt".into())
            }
            CliError::StaleHandle(_) => Some(
                "@N handles refer to your last list and expire after an hour; re-run the \
                 list command"
                    .into(),
            ),
            _ => None,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for CliError {}

impl From<anyhow::Error> for CliError {
    fn from(err: anyhow::Error) -> Self {
        CliError::Other(err)
    }
}

impl From<std::io::Error> for CliError {
    fn from(err: std::io::Error) -> Self {
        CliError::Other(err.into())
    }
}

impl From<serde_json::Error> for CliError {
    fn from(err: serde_json::Error) -> Self {
        CliError::Other(err.into())
    }
}

pub type CliResult<T> = Result<T, CliError>;

/// Shorthand for the common "this is just wrong usage" case.
pub fn usage<T>(msg: impl Into<String>) -> CliResult<T> {
    Err(CliError::Usage(msg.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(code: &str) -> CliError {
        CliError::Api(ApiErrorBody {
            code: code.into(),
            message: "boom".into(),
            status: 500,
            req_id: "01JREQ".into(),
        })
    }

    #[test]
    fn exit_codes_match_the_documented_table() {
        assert_eq!(
            CliError::Unreachable {
                url: "http://x".into(),
                detail: "connection refused".into()
            }
            .exit_code(),
            12
        );
        assert_eq!(CliError::Degraded("x".into()).exit_code(), 13);
        assert_eq!(CliError::StaleHandle("x".into()).exit_code(), 14);
        assert_eq!(CliError::PartialFailure("x".into()).exit_code(), 11);
        assert_eq!(CliError::NeedsConfirmation("x".into()).exit_code(), 10);
        assert_eq!(CliError::Usage("x".into()).exit_code(), 2);
    }

    #[test]
    fn a_missing_document_exits_3_not_1() {
        // `ovis page view <typo>` has to be distinguishable from a server error.
        assert_eq!(api("NOT_FOUND").exit_code(), exit::NOT_FOUND);
        assert_eq!(api("BAD_REQUEST").exit_code(), exit::USAGE);
        assert_eq!(api("DATABASE").exit_code(), exit::GENERIC);
    }

    #[test]
    fn api_errors_render_code_and_request_id_so_a_failure_can_be_traced() {
        let rendered = api("DATABASE").message();
        assert_eq!(rendered, "boom (code DATABASE, req 01JREQ)");
        assert!(api("DATABASE").hint().unwrap().contains("01JREQ"));
    }

    #[test]
    fn unreachable_hints_at_starting_the_server() {
        let err = CliError::Unreachable {
            url: "http://127.0.0.1:8080".into(),
            detail: "connection refused".into(),
        };
        assert!(err.message().contains("cannot reach OVIS server"));
        assert!(err.hint().unwrap().contains("ovis server start"));
    }

    #[test]
    fn a_parked_connector_is_told_which_flag_unblocks_it() {
        assert!(api("PARKED_CONNECTOR")
            .hint()
            .unwrap()
            .contains("--acknowledge-parked"));
    }

    #[test]
    fn an_html_error_page_is_reported_as_an_unexpected_shape_not_a_parse_failure() {
        let err = CliError::Http {
            status: 502,
            body: "<html><body>Bad Gateway</body></html>".into(),
        };
        assert!(err.message().contains("502"));
        assert!(err.message().contains("Bad Gateway"));
    }
}
