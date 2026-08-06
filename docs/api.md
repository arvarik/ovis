# HTTP API

Base path `/api/v1`. JSON everywhere except SSE (`/pages/stream`) and
`/pages/{id}/text` (plain text). This page documents the API as shipped.

## Conventions

**Document ids are URLs and occupy exactly one path segment** — clients
percent-encode them (`encodeURIComponent`-style):

```bash
curl "localhost:8080/api/v1/pages/https%3A%2F%2Fexample.com%2Fa"
```

An unencoded id matches no route and gets a 400 naming the fix, rather than
falling through to the SPA and returning HTML to a JSON client.

**Unknown query parameters are rejected** (`400 UNKNOWN_PARAM`) — typos fail
loudly instead of silently filtering nothing.

**Every non-2xx carries the same envelope:**

```json
{ "error": { "code": "DATABASE", "message": "database error", "status": 500, "req_id": "ms2dmisf0004" } }
```

Branch on `code`. `req_id` also arrives as the `x-request-id` header and is on
every server log line for that request. Codes are stable per failure class:
`NOT_FOUND`, `BAD_REQUEST`, `UNKNOWN_PARAM`, `UNAUTHORIZED`, `CONFLICT`,
`PARKED_CONNECTOR`, `ONYX_UPSTREAM`, `OPENSEARCH_UPSTREAM`, `EMBED_UPSTREAM`,
`DATABASE`, `TIMEOUT`, `ONYX_UNCONFIGURED`, `SCHEMA_MISMATCH`.

**Auth.** With `OVIS_API_TOKEN` set, every route requires
`Authorization: Bearer <token>`; `/system/health` stays open for container
probes, and SSE also accepts `?token=` (EventSource cannot send headers).

**Pagination** is uniform across list endpoints:

```json
{ "items": [...], "total": 1646781, "total_exact": true,
  "page": 1, "limit": 50, "next_cursor": "eyJ…", "has_more": true }
```

Requests take `limit` plus either `page` (1-based, refused past
`page*limit > 50000`) **or** `cursor` (opaque keyset token, no depth limit).
Exception: `GET /connectors` returns a bare array — the fleet is small and
unpaginated. Attempt/error lists paginate by `page` (their `next_cursor` is
always null).

## Pages

```
GET    /pages                     search, connector_id, source, hidden,
                                  chunk_min/chunk_max, updated_after/updated_before,
                                  sort = updated_desc|updated_asc|chunks_desc|chunks_asc|
                                         id_asc|id_desc|boost_desc,
                                  limit, page|cursor
GET    /pages/stream              same filters, as SSE (see below)
GET    /pages/{id}                metadata detail — no chunk content, <25 ms
PATCH  /pages/{id}                { semantic_id?, boost?, hidden?, metadata_merge? }
DELETE /pages/{id}                cascading delete
POST   /pages/batch-delete        { "document_ids": [...] }  (≤ OVIS_BATCH_DELETE_MAX)
GET    /pages/{id}/chunks         limit, after (chunk-index cursor); never vectors
GET    /pages/{id}/chunks/{n}/vector   one real stored vector { dim, model, vector }
GET    /pages/{id}/text           text/plain reconstructed; ?download=1 for attachment
```

Notes:

- `updated_at` is the effective, never-null recency stamp
  (`COALESCE(doc_updated_at, last_modified)`) and exactly what
  `sort=updated_*` orders by. `doc_updated_at` is null for the overwhelming
  majority of crawled rows — don't build on it.
- `PATCH` merges `metadata_merge` shallowly (top-level keys replace); it never
  stomps the whole object. When an Onyx token is configured, boost/hidden are
  proxied through Onyx (`boost_hidden_via: "onyx_api"`) so its own index stays
  in sync; otherwise applied directly (`"direct_sql"`). The response includes
  `index_synced` for title changes.
- `DELETE` responds `{ pg_deleted, chunks_deleted, index_cleanup_pending,
  recrawl_risk }` — see [the honest fields](#the-honest-fields). Batch delete
  reports per-id failures; `success` is true only when `failed` is empty.

### SSE contract (`GET /pages/stream`)

`event: page` per row (with `id:` set), heartbeat comment `:ka` every 15 s,
terminal `event: done` with `{"total_matched": n, "time_ms": t}`, failure
`event: error` with the envelope's code/message. **The stream is finite**:
default `limit` 1000, ceiling `OVIS_MAX_STREAM_LIMIT` (10,000). Compare rows
received against `done.total_matched` — a capped stream is not the whole set,
and both shipped clients say so rather than pretending.

## Content search

```
GET /search    q (required), mode = keyword|semantic|hybrid (default keyword),
               connector_id, source, limit (≤100), offset (≤1000)
```

```json
{ "items": [ { "document_id": "…", "score": 13.3, "snippet": "…<em>match</em>…", … } ],
  "mode": "hybrid", "degraded": "no_knn_field",
  "total_hits": 10000, "total_hits_exact": false, "took_ms": 29 }
```

One result per document (collapsed), snippets highlighted with `<em>`.
**`mode` echoes what was requested** — a degraded hybrid still says `hybrid`.
Key off `degraded`, an **open string**: values seen in the wild are
`no_knn_field` (index can't serve kNN yet), `no_embedder` (no embedding
endpoint configured/reachable), and `connector_filter_post_applied`
(connector scope applied after ranking; totals inexact). Render unknown values
verbatim rather than dropping them.

## Connectors

```
GET    /connectors                          bare array of summaries: real status,
                                            dcc-derived doc_count, parked, last_attempt
GET    /connectors/{cc_pair_id}             + config, credential name (never secrets),
                                            attempt aggregates; ?history=7d adds
                                            [{day, docs_added}] — detail only
GET    /connectors/{cc_pair_id}/attempts    paginated attempt telemetry
GET    /connectors/{cc_pair_id}/errors      rolling window; response carries "window":"24h"
GET    /connectors/{cc_pair_id}/docs        the pair's authoritative doc list (dcc join)
POST   /connectors/{cc_pair_id}/pause       → Onyx
POST   /connectors/{cc_pair_id}/resume      → Onyx
POST   /connectors/{cc_pair_id}/run-once    { from_beginning?, acknowledge_parked? }
POST   /connectors/{cc_pair_id}/prune       → Onyx
PATCH  /connectors/{cc_pair_id}             { name?, refresh_freq_secs? }
DELETE /connectors/{cc_pair_id}             { "confirm_name": "<exact name>" }
```

- `doc_count` is counted from `document_by_connector_credential_pair` —
  Onyx's own `total_docs_indexed` column is unreliable and never used.
- `parked: true` means the latest attempt carries a resilience-cron sentinel
  (`first-pass already complete` / `park done`): the crawl was *finished on
  purpose*. `run-once` on a parked pair answers `409 PARKED_CONNECTOR` unless
  the body sets `"acknowledge_parked": true` — a deliberate human override,
  which no client sets automatically.
- Connector delete destroys every document the pair owns; the exact-name echo
  is the guard against fat-fingering an id. All actions answer
  `503 ONYX_UNCONFIGURED` without a token, and are audit-logged.

## Indexing activity

```
GET  /indexing/attempts             ?status= filters; global attempt telemetry
GET  /indexing/background-errors
GET  /indexing/failed-documents
POST /indexing/targeted-reindex     { cc_pair_id, document_ids? | only_failed? }
GET  /indexing/targeted-reindex/{job_id}
GET  /indexing/background-errors    ?cc_pair_id=&limit= — errors the workers recorded
                                    outside any attempt, so no attempt lists them
GET  /indexing/failed-documents     Onyx's own view, proxied verbatim
```

Attempt items carry derived truth: `stalled` (IN_PROGRESS with no heartbeat
for 45 minutes — the resilience cron's own heuristic, never doc counts:
healthy connectors legitimately sit at zero docs) and `pages_per_min` for
running attempts. A `NOT_STARTED` attempt is normal queueing, not a stall.

## Pruning

The review-first junk-removal lifecycle. Scans preview, staging is the
reversible `hidden` primitive with a server-side grace deadline, and hard
deletion happens only in the background reaper. Full guide:
[pruning.md](./pruning.md).

```
GET    /prune/status                      counts per state, reaper state, limits
POST   /prune/scans                       { scope, detectors[], config_overrides? } → 202
GET    /prune/scans                       history; GET /prune/scans/{id} is the poll target
POST   /prune/scans/{id}/cancel           stops at the next checkpoint

GET    /prune/candidates                  state, detector, connector_id, min_confidence,
                                          recrawl_risk, scan_id, sort, limit, page
GET    /prune/candidates/{id}             + hydrated duplicate-pair evidence
POST   /prune/candidates/stage            { ids|filter, confirm_count } → hidden, grace starts
POST   /prune/candidates/dismiss          { ids|filter, exclude_future }
POST   /prune/candidates/restore          { ids|filter }  → exact un-stage
POST   /prune/candidates/schedule-delete  { ids|filter, confirm_count, remember? }

GET/POST /prune/rules                     PATCH/DELETE /prune/rules/{id}
POST   /prune/rules/{id}/preview          sample matches, live data, zero mutation
GET    /prune/config                      effective detector config as YAML
PUT    /prune/config                      import YAML (validated, stored as a rule)
GET    /prune/audit                       action, actor, document_id, since, limit, page
GET    /prune/exclusions                  DELETE /prune/exclusions/{id}
```

Contract notes:

- **Every bulk mutation resolves its selection first and compares
  `confirm_count`**; a drifted set is `409 CONFLICT` whose message carries
  the fresh count, and nothing changes. `filter` mirrors the GET filters.
- Bulk responses report per-id outcomes (`failed[]` with codes; `success`
  only when it is empty; HTTP 207 on partial failure) plus
  `boost_hidden_via` so a direct-SQL fallback is visible.
- `chunk_count: null` on a candidate means "not counted yet", never "empty" —
  same as everywhere else.
- Scan `detectors[]` names what runs; nothing runs unasked:
  `exact_duplicate`, `near_duplicate`, `url_variant`, `url_junk`, `thin`,
  `quality`, `language`, `url_rule`, `tag_rule`, `stale`. Candidate
  `detector` filtering uses the *reason* vocabulary instead (`duplicate`,
  `thin`, `quality`, `url_junk`, `language`, `url_rule`, `tag_rule`, `stale`,
  `recrawl`).
- `/prune/*` answers `501 NOT_AVAILABLE` when the `ovis.prune_*` tables could
  not be created at startup.

### Measurements and policy

A scan records what it *measured* (`ovis.doc_profile`, `ovis.dup_pair`,
`ovis.doc_dup_group`); a policy turns measurements into `auto` / `review` /
untouched bands at read time. That split is what makes a threshold change a
query rather than a re-scan.

```
GET  /prune/overview                      the funnel: bundles by reason, by connector,
                                          corpus totals, trash counts
GET  /prune/histogram?signal=&buckets=    distribution of one measured column;
                                          signal ∈ max_jaccard, max_cosine,
                                          quality_fail_count, word_count, centroid_pct
POST /prune/simulate                      { tier | policy, sample? } → band counts,
                                          per-signal and per-connector breakdown,
                                          random members of each band, caveats
GET  /prune/policies                      saved threshold sets, active one flagged
POST /prune/policies/commit               { tier | policy, band, confirm_count, save_as? }
GET  /prune/clusters?method=&after=&limit= duplicate groups, keeper first;
                                          method ∈ hash, url; `after` is a key cursor
GET  /prune/sample?detector=&code=&n=     acceptance-sampling draw + its claim
POST /prune/narrate                       { subject_kind, method?, limit? } generated titles
```

- **Simulating mutates nothing** — it is an aggregate over stored
  measurements, safe on every request.
- Committing creates *candidates only*. Staging is still a separate confirmed
  action and deletion still waits out the grace period.
- `caveats[]` states what the numbers cannot cover — a policy with semantic
  thresholds and no measured embedding similarity says so rather than
  reporting a confident zero.
- `sample` is drawn server-side so a client cannot cherry-pick an easy one,
  and `statement` carries the confidence bound in words.

### Trash

What the reaper deletes is snapshotted first — Postgres rows, tags, connector
attribution and every chunk's verbatim `_source` including embedding vectors.

```
GET    /prune/trash                       connector_id, document_id, hold,
                                          expiring_within_days, limit, page
GET    /prune/trash/{id}                  the snapshot: text, chunk previews, provenance
POST   /prune/trash/restore               { ids|filter, confirm_count?, overwrite? }
POST   /prune/trash/purge                 { ids|filter, typed_count } — irreversible
POST   /prune/trash/hold                  { document_ids[], hold } — exempt from auto-purge
```

- Restore re-inserts the rows and bulk-indexes the chunks under their original
  ids, so a restored document is semantically searchable immediately.
  `reappeared: true` means the crawler brought the id back; restoring then
  requires `overwrite`.
- **Purge demands `typed_count` at every size**, not just past the bulk
  threshold, and skips held snapshots. There is deliberately no "empty trash".

## LLM providers

Optional. Nothing in pruning requires a model; this adds relevance judging and
the generated titles on cluster and bundle cards.

```
GET/POST /llm/providers                   connect an endpoint (llamacpp, ollama,
                                          openai_compatible, gemini, anthropic)
DELETE   /llm/providers/{id}
POST     /llm/providers/{id}/discover     re-read the catalogue, keeping role assignments
POST     /llm/providers/{id}/probe        probe every model this provider serves
GET      /llm/models                      ?provider_id=
POST     /llm/models/{provider_id}/probe  { model_id } → measured capabilities
GET/PUT  /llm/roles                       assign bulk / quality / narrate
```

A provider's own listing says a model *exists*; only a probe says whether its
output constraints actually hold. A model that has not passed one cannot be
given a role — the server refuses, it is not merely hidden in the UI.

## Tags, stats, system

```
GET /tags?key=&limit=               facet counts (cached 60 s)
GET /tags/keys?limit=               tag keys with their distinct-value counts
GET /stats/overview                 documents, chunks, connector counts, index+disk,
                                    embedding info, crawl rates, attempt outcomes
GET /stats/index                    index size/docs/disk/read-only only — the cheap
                                    health probe: OpenSearch alone, no Postgres
GET /stats/timeline?window=24h|7d|30d&bucket=1h|1d
GET /stats/sources                  per-source docs/chunks/connector counts
GET /stats/connectors/top?by=docs|recent&limit=
GET /system/health                  full dependency report; HTTP 503 when degraded
GET /system/runtime                 index name, embedding model/dim, query prefix,
                                    search-settings id, schema flag, refresh time
GET /system/version                 version, git sha, rustc, built-at, profile
GET /system/metrics                 Prometheus text
```

`/stats/index` is the same object `/stats/overview` nests under `index`, but
reached without the six Postgres aggregates the overview also runs — the
endpoint to poll from a monitor that only wants disk headroom and the
read-only alarm.

The Onyx version lives on `/system/health` (`onyx_api.version`), **not** on
`/system/runtime`.

## The honest fields

The API's defining feature: it states uncertainty and risk instead of
smoothing them over. Clients are expected to render these, not hide them.

| Field | Meaning |
|---|---|
| `total_exact: false` | the total is a planner estimate (only ever the unfiltered grand total; an exact count computes in the background and takes over) |
| `chunk_count: null` | Onyx has not counted this document yet — **not** the same as 0; `chunk_min`/`chunk_max` deliberately exclude such rows |
| `degraded` | search fell back or filtered post-hoc; open string, render verbatim |
| `pg_row: false` | the index holds chunks for a document with no Postgres row — orphaned chunks, a cleanup candidate |
| `recrawl_risk: true` | the owning cc-pair is ACTIVE/INITIAL_INDEXING; a delete will likely be undone at the next scheduled refresh |
| `index_cleanup_pending: true` | Postgres committed but the index delete couldn't be confirmed; the id is queued and retried — no silent orphans |
| `parked: true` | the resilience cron finished this cc-pair on purpose |
| `stalled: true` | IN_PROGRESS with no heartbeat for 45 minutes |
| `in_repeated_error_state` | Onyx's own repeated-failure flag, surfaced |
| `window: "24h"` | error lists are a rolling window — empty ≠ "no failures ever" |
