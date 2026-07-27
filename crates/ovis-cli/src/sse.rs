//! A minimal Server-Sent Events reader for `--all`.
//!
//! The stream contract is in `backend/03_API_SURFACE.md` and the handler that
//! implements it: `event: page` per row, `:ka` comments every 15 s, a final
//! `event: done`, and `event: error` carrying the same envelope as an HTTP
//! error. That last one is the reason this parses events rather than just
//! splitting on `data:` — a stream that dies half way through has to fail the
//! command, not truncate the output and exit 0.

use futures_util::StreamExt;

use crate::error::{ApiErrorBody, CliError, CliResult};

#[derive(Debug, Clone, PartialEq)]
pub enum SseEvent {
    Page(String),
    Done(String),
    Error(ApiErrorBody),
}

/// Incremental parser: feed it bytes, take whole events out.
#[derive(Debug, Default)]
pub struct SseParser {
    buffer: String,
    event: Option<String>,
    data: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &str) -> Vec<SseEvent> {
        self.buffer.push_str(chunk);
        let mut out = Vec::new();

        // Only consume complete lines; a partial line stays buffered for the
        // next chunk. Splitting on '\n' mid-JSON is exactly how naive readers
        // corrupt long documents.
        while let Some(newline) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=newline).collect();
            let line = line.trim_end_matches(['\n', '\r']);

            if line.is_empty() {
                if let Some(event) = self.take_event() {
                    out.push(event);
                }
                continue;
            }
            // `:ka` keep-alives, which exist so proxies do not reap the stream.
            if line.starts_with(':') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("event:") {
                self.event = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            }
            // `id:` and `retry:` are part of the protocol but carry nothing this
            // client acts on.
        }
        out
    }

    fn take_event(&mut self) -> Option<SseEvent> {
        let data = std::mem::take(&mut self.data);
        let name = self.event.take();
        if data.is_empty() {
            return None;
        }
        match name.as_deref() {
            Some("page") => Some(SseEvent::Page(data)),
            Some("done") => Some(SseEvent::Done(data)),
            Some("error") => Some(SseEvent::Error(
                serde_json::from_str::<ApiErrorBody>(&data).unwrap_or(ApiErrorBody {
                    code: "STREAM_ERROR".into(),
                    message: data,
                    status: 500,
                    req_id: String::new(),
                }),
            )),
            _ => None,
        }
    }
}

/// Drive a stream to completion, handing each row's raw JSON to `on_page`.
///
/// Returns the number of rows emitted. An `event: error` becomes a `CliError`,
/// so a stream that dies half way through fails the command.
pub async fn consume<F>(response: reqwest::Response, mut on_page: F) -> CliResult<u64>
where
    F: FnMut(&str) -> CliResult<()>,
{
    let mut parser = SseParser::new();
    let mut stream = response.bytes_stream();
    let mut emitted: u64 = 0;
    let mut saw_done = false;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|e| CliError::Other(anyhow::anyhow!("the stream broke mid-flight: {e}")))?;
        let text = String::from_utf8_lossy(&chunk);
        for event in parser.feed(&text) {
            match event {
                SseEvent::Page(data) => {
                    on_page(&data)?;
                    emitted += 1;
                }
                SseEvent::Done(_) => saw_done = true,
                SseEvent::Error(body) => return Err(CliError::Api(body)),
            }
        }
        if saw_done {
            break;
        }
    }

    if !saw_done {
        // The server always sends `done` last on success, so its absence means
        // the connection dropped. Reporting the rows we got as a complete answer
        // would be the same class of lie as the sample-data fallback.
        return Err(CliError::Other(anyhow::anyhow!(
            "the stream ended after {emitted} rows without a completion event; the result is \
             incomplete"
        )));
    }
    Ok(emitted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_complete_event_is_parsed() {
        let mut parser = SseParser::new();
        let events = parser.feed("event: page\nid: 0\ndata: {\"id\":\"a\"}\n\n");
        assert_eq!(events, vec![SseEvent::Page("{\"id\":\"a\"}".into())]);
    }

    #[test]
    fn events_split_across_chunk_boundaries_are_reassembled() {
        // The whole reason for an incremental parser: a 4 KB read can land
        // anywhere, including the middle of a JSON string.
        let mut parser = SseParser::new();
        assert!(parser.feed("event: pa").is_empty());
        assert!(parser.feed("ge\ndata: {\"id\":\"htt").is_empty());
        assert!(parser.feed("ps://x/y\"}").is_empty());
        let events = parser.feed("\n\n");
        assert_eq!(
            events,
            vec![SseEvent::Page("{\"id\":\"https://x/y\"}".into())]
        );
    }

    #[test]
    fn keepalive_comments_are_ignored() {
        let mut parser = SseParser::new();
        assert!(parser.feed(":ka\n\n").is_empty());
        let events = parser.feed(":ka\nevent: page\ndata: {}\n\n");
        assert_eq!(events, vec![SseEvent::Page("{}".into())]);
    }

    #[test]
    fn several_events_arrive_in_order_from_one_chunk() {
        let mut parser = SseParser::new();
        let events = parser.feed(
            "event: page\ndata: 1\n\nevent: page\ndata: 2\n\nevent: done\ndata: {\"total_matched\":2}\n\n",
        );
        assert_eq!(
            events,
            vec![
                SseEvent::Page("1".into()),
                SseEvent::Page("2".into()),
                SseEvent::Done("{\"total_matched\":2}".into()),
            ]
        );
    }

    #[test]
    fn an_error_event_carries_the_http_error_envelope() {
        let mut parser = SseParser::new();
        let events = parser.feed(
            "event: error\ndata: {\"code\":\"DATABASE\",\"message\":\"database error\",\
             \"status\":500,\"req_id\":\"01J\"}\n\n",
        );
        match &events[0] {
            SseEvent::Error(body) => {
                assert_eq!(body.code, "DATABASE");
                assert_eq!(body.req_id, "01J");
            }
            other => panic!("expected an error event, got {other:?}"),
        }
    }

    #[test]
    fn a_malformed_error_payload_still_fails_the_stream() {
        let mut parser = SseParser::new();
        let events = parser.feed("event: error\ndata: not json\n\n");
        match &events[0] {
            SseEvent::Error(body) => assert_eq!(body.code, "STREAM_ERROR"),
            other => panic!("expected an error event, got {other:?}"),
        }
    }

    #[test]
    fn crlf_line_endings_are_handled() {
        let mut parser = SseParser::new();
        let events = parser.feed("event: page\r\ndata: {}\r\n\r\n");
        assert_eq!(events, vec![SseEvent::Page("{}".into())]);
    }

    #[test]
    fn multi_line_data_fields_are_joined_with_newlines_per_the_spec() {
        let mut parser = SseParser::new();
        let events = parser.feed("event: page\ndata: a\ndata: b\n\n");
        assert_eq!(events, vec![SseEvent::Page("a\nb".into())]);
    }
}
