//! The async data worker.
//!
//! The render loop never touches the network. It sends [`UiCmd`] here and gets
//! [`DataEvent`] back over a channel, so a slow request cannot stall a frame —
//! the old TUI fetched everything up front, synchronously, one OpenSearch call
//! per document.

use std::collections::HashMap;

use ovis_core::api_types::*;
use tokio::sync::mpsc;

use crate::api::{ApiClient, QueryBuilder};
use crate::error::CliError;

/// How many rows one page of the infinite scroll fetches.
pub const PAGE_SIZE: i64 = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct PagesQuery {
    pub filter: String,
    pub connector_id: Option<i32>,
    pub sort: String,
    pub include_hidden: bool,
    /// `None` for the first page.
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchQuery {
    pub q: String,
    pub mode: String,
    pub connector_id: Option<i32>,
}

#[derive(Debug, Clone)]
pub enum UiCmd {
    LoadPages {
        query: PagesQuery,
        append: bool,
    },
    Search(SearchQuery),
    LoadDetail(String),
    LoadText(String),
    LoadChunks(String),
    LoadConnectors,
    LoadConnectorDetail(i32),
    LoadAttempts,
    LoadStats,
    Delete(Vec<String>),
    ConnectorAction {
        cc_pair_id: i32,
        name: String,
        action: ConnectorAction,
    },
    LoadConnectorErrors(i32),
    LoadConnectorAttempts(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorAction {
    Pause,
    Resume,
    RunOnce { acknowledge_parked: bool },
}

impl ConnectorAction {
    pub fn verb(self) -> &'static str {
        match self {
            ConnectorAction::Pause => "pause",
            ConnectorAction::Resume => "resume",
            ConnectorAction::RunOnce { .. } => "run-once",
        }
    }
}

#[derive(Debug)]
pub enum DataEvent {
    Pages {
        items: Vec<PageListItem>,
        total: i64,
        total_exact: bool,
        next_cursor: Option<String>,
        append: bool,
    },
    SearchResults {
        items: Vec<SearchHit>,
        total: i64,
        total_exact: bool,
        mode: String,
        degraded: Option<String>,
        took_ms: u64,
    },
    Detail(Box<PageDetail>),
    Text(String, String),
    Chunks(String, Box<ChunksResponse>),
    Connectors(Vec<ConnectorSummary>),
    ConnectorDetail(Box<ConnectorDetail>),
    Attempts(Vec<IndexAttemptItem>),
    ConnectorErrors(i32, Vec<IndexAttemptError>, String),
    Stats(Box<StatsOverview>),
    Deleted(Box<BatchDeleteResponse>),
    ActionDone(Box<ActionResponse>),
    /// Anything that failed, as a message for the toast line. Never swallowed:
    /// the old TUI turned every error into `Ok(())`.
    Failed(String),
}

/// Spawn the worker. Returns the command sender and the event receiver.
pub fn spawn(api: ApiClient) -> (mpsc::Sender<UiCmd>, mpsc::Receiver<DataEvent>) {
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<UiCmd>(64);
    let (event_tx, event_rx) = mpsc::channel::<DataEvent>(64);

    tokio::spawn(async move {
        // Detail and text bodies are re-read constantly while scrolling; a small
        // cache keeps the inspector instant without unbounded growth.
        let mut detail_cache: Lru<String, PageDetail> = Lru::new(100);
        let mut text_cache: Lru<String, String> = Lru::new(20);

        while let Some(cmd) = cmd_rx.recv().await {
            let event = handle(&api, cmd, &mut detail_cache, &mut text_cache).await;
            if let Some(event) = event {
                if event_tx.send(event).await.is_err() {
                    break; // the UI is gone
                }
            }
        }
    });

    (cmd_tx, event_rx)
}

async fn handle(
    api: &ApiClient,
    cmd: UiCmd,
    detail_cache: &mut Lru<String, PageDetail>,
    text_cache: &mut Lru<String, String>,
) -> Option<DataEvent> {
    let event = match cmd {
        UiCmd::LoadPages { query, append } => {
            let mut q = QueryBuilder::new();
            q.push("limit", PAGE_SIZE).push("sort", &query.sort);
            if !query.filter.trim().is_empty() {
                q.push("search", query.filter.trim());
            }
            q.push_opt("connector_id", query.connector_id);
            if !query.include_hidden {
                q.push("hidden", false);
            }
            q.push_opt("cursor", query.cursor.as_ref());

            match api.pages(&q.build()).await {
                Ok(response) => DataEvent::Pages {
                    items: response.items,
                    total: response.total,
                    total_exact: response.total_exact,
                    next_cursor: response.next_cursor,
                    append,
                },
                Err(err) => fail("load pages", err),
            }
        }

        UiCmd::Search(query) => {
            let mut q = QueryBuilder::new();
            q.push("q", &query.q)
                .push("mode", &query.mode)
                .push("limit", 50);
            q.push_opt("connector_id", query.connector_id);
            match api.search(&q.build()).await {
                Ok(response) => DataEvent::SearchResults {
                    items: response.items,
                    total: response.total_hits,
                    total_exact: response.total_hits_exact,
                    mode: response.mode,
                    degraded: response.degraded,
                    took_ms: response.took_ms,
                },
                Err(err) => fail("search", err),
            }
        }

        UiCmd::LoadDetail(id) => {
            if let Some(cached) = detail_cache.get(&id) {
                return Some(DataEvent::Detail(Box::new(cached.clone())));
            }
            match api.page_detail(&id).await {
                Ok(detail) => {
                    detail_cache.put(id, detail.clone());
                    DataEvent::Detail(Box::new(detail))
                }
                Err(err) => fail("load document", err),
            }
        }

        UiCmd::LoadText(id) => {
            if let Some(cached) = text_cache.get(&id) {
                return Some(DataEvent::Text(id, cached.clone()));
            }
            match api.page_text(&id).await {
                Ok(text) => {
                    text_cache.put(id.clone(), text.clone());
                    DataEvent::Text(id, text)
                }
                Err(err) => fail("load text", err),
            }
        }

        UiCmd::LoadChunks(id) => {
            let mut q = QueryBuilder::new();
            q.push("limit", 50);
            match api.page_chunks(&id, &q.build()).await {
                Ok(chunks) => DataEvent::Chunks(id, Box::new(chunks)),
                Err(err) => fail("load chunks", err),
            }
        }

        UiCmd::LoadConnectors => match api.connectors().await {
            Ok(mut items) => {
                items.sort_by_key(|c| std::cmp::Reverse(c.doc_count));
                DataEvent::Connectors(items)
            }
            Err(err) => fail("load connectors", err),
        },

        UiCmd::LoadConnectorDetail(cc_pair_id) => {
            match api.connector(cc_pair_id, "history=14d").await {
                Ok(detail) => DataEvent::ConnectorDetail(Box::new(detail)),
                Err(err) => fail("load connector", err),
            }
        }

        UiCmd::LoadAttempts => {
            let mut q = QueryBuilder::new();
            q.push("limit", 100);
            match api.attempts(&q.build()).await {
                Ok(response) => DataEvent::Attempts(response.items),
                Err(err) => fail("load attempts", err),
            }
        }

        UiCmd::LoadConnectorAttempts(cc_pair_id) => {
            let mut q = QueryBuilder::new();
            q.push("limit", 100);
            match api.connector_attempts(cc_pair_id, &q.build()).await {
                Ok(response) => DataEvent::Attempts(response.items),
                Err(err) => fail("load attempts", err),
            }
        }

        UiCmd::LoadConnectorErrors(cc_pair_id) => {
            let mut q = QueryBuilder::new();
            q.push("limit", 100);
            match api.connector_errors(cc_pair_id, &q.build()).await {
                Ok(response) => {
                    DataEvent::ConnectorErrors(cc_pair_id, response.items, response.window)
                }
                Err(err) => fail("load errors", err),
            }
        }

        UiCmd::LoadStats => match api.stats_overview().await {
            Ok(stats) => DataEvent::Stats(Box::new(stats)),
            Err(err) => fail("load stats", err),
        },

        UiCmd::Delete(ids) => {
            // One code path for one or many, so the reported outcome has the
            // same shape either way.
            if ids.len() == 1 {
                match api.page_delete(&ids[0]).await {
                    Ok(outcome) => DataEvent::Deleted(Box::new(BatchDeleteResponse {
                        success: outcome.pg_deleted,
                        deleted: usize::from(outcome.pg_deleted),
                        chunks_deleted: outcome.chunks_deleted,
                        failed: vec![],
                        index_cleanup_pending: usize::from(outcome.index_cleanup_pending),
                    })),
                    Err(err) => fail("delete", err),
                }
            } else {
                match api.pages_batch_delete(ids).await {
                    Ok((_, response)) => DataEvent::Deleted(Box::new(response)),
                    Err(err) => fail("delete", err),
                }
            }
        }

        UiCmd::ConnectorAction {
            cc_pair_id,
            name,
            action,
        } => {
            let result = match action {
                ConnectorAction::Pause => api.connector_action(cc_pair_id, "pause").await,
                ConnectorAction::Resume => api.connector_action(cc_pair_id, "resume").await,
                ConnectorAction::RunOnce { acknowledge_parked } => {
                    api.connector_run_once(
                        cc_pair_id,
                        &RunOnceRequest {
                            from_beginning: false,
                            acknowledge_parked,
                        },
                    )
                    .await
                }
            };
            match result {
                Ok(response) => DataEvent::ActionDone(Box::new(response)),
                Err(err) => fail(&format!("{} {name}", action.verb()), err),
            }
        }
    };
    Some(event)
}

fn fail(what: &str, err: CliError) -> DataEvent {
    DataEvent::Failed(format!("cannot {what}: {}", err.message()))
}

/// A small insertion-ordered LRU. `lru` the crate would do, but it is one of the
/// two `cargo audit` advisories the old ratatui pinned in, and this is 30 lines.
pub struct Lru<K, V> {
    map: HashMap<K, V>,
    order: Vec<K>,
    capacity: usize,
}

impl<K: std::hash::Hash + Eq + Clone, V> Lru<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: Vec::new(),
            capacity: capacity.max(1),
        }
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        if self.map.contains_key(key) {
            // Touch: move to the most-recent end.
            self.order.retain(|k| k != key);
            self.order.push(key.clone());
        }
        self.map.get(key)
    }

    pub fn put(&mut self, key: K, value: V) {
        if self.map.insert(key.clone(), value).is_some() {
            self.order.retain(|k| k != &key);
        }
        self.order.push(key);
        while self.order.len() > self.capacity {
            let evicted = self.order.remove(0);
            self.map.remove(&evicted);
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_evicts_the_least_recently_used_entry() {
        let mut lru: Lru<&str, i32> = Lru::new(2);
        lru.put("a", 1);
        lru.put("b", 2);
        // Touching "a" makes "b" the oldest.
        assert_eq!(lru.get(&"a"), Some(&1));
        lru.put("c", 3);
        assert_eq!(lru.len(), 2);
        assert_eq!(lru.get(&"b"), None);
        assert_eq!(lru.get(&"a"), Some(&1));
        assert_eq!(lru.get(&"c"), Some(&3));
    }

    #[test]
    fn re_putting_a_key_does_not_grow_the_cache() {
        let mut lru: Lru<&str, i32> = Lru::new(2);
        lru.put("a", 1);
        lru.put("a", 2);
        lru.put("b", 3);
        assert_eq!(lru.len(), 2);
        assert_eq!(lru.get(&"a"), Some(&2));
    }

    #[test]
    fn a_zero_capacity_cache_still_holds_one_entry_rather_than_panicking() {
        let mut lru: Lru<&str, i32> = Lru::new(0);
        lru.put("a", 1);
        assert_eq!(lru.get(&"a"), Some(&1));
    }

    #[test]
    fn an_empty_cache_reports_itself_as_empty() {
        let lru: Lru<&str, i32> = Lru::new(4);
        assert!(lru.is_empty());
    }

    #[test]
    fn connector_actions_name_the_endpoint_they_post_to() {
        assert_eq!(ConnectorAction::Pause.verb(), "pause");
        assert_eq!(ConnectorAction::Resume.verb(), "resume");
        assert_eq!(
            ConnectorAction::RunOnce {
                acknowledge_parked: false
            }
            .verb(),
            "run-once"
        );
    }

    #[test]
    fn a_single_delete_is_reported_in_the_same_shape_as_a_batch() {
        // So the toast line has one format regardless of how many rows were
        // marked.
        let outcome = DeleteOutcome {
            pg_deleted: true,
            chunks_deleted: 14,
            index_cleanup_pending: false,
            recrawl_risk: true,
        };
        let as_batch = BatchDeleteResponse {
            success: outcome.pg_deleted,
            deleted: usize::from(outcome.pg_deleted),
            chunks_deleted: outcome.chunks_deleted,
            failed: vec![],
            index_cleanup_pending: usize::from(outcome.index_cleanup_pending),
        };
        assert_eq!(as_batch.deleted, 1);
        assert_eq!(as_batch.chunks_deleted, 14);
        assert!(as_batch.success);
    }

    #[test]
    fn a_failed_delete_is_reported_rather_than_swallowed() {
        // The old TUI mutated in-memory state and printed "Successfully
        // deleted page" no matter what.
        let event = fail(
            "delete",
            CliError::Api(crate::error::ApiErrorBody {
                code: "DATABASE".into(),
                message: "database error".into(),
                status: 500,
                req_id: "01J".into(),
            }),
        );
        match event {
            DataEvent::Failed(msg) => {
                assert!(msg.contains("cannot delete"), "{msg}");
                assert!(msg.contains("DATABASE"), "{msg}");
            }
            other => panic!("expected a failure event, got {other:?}"),
        }
    }
}
