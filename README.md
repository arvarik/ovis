# OVIS — Onyx Visibility

OVIS surfaces and controls the pages crawled by a distributed [Onyx](https://github.com/onyx-dot-app/onyx)
(formerly Danswer) deployment. Onyx crawls and answers questions well, but its raw
page store is effectively invisible: you cannot easily see what it holds, why a
connector stopped, or what a given document actually looks like in the index. OVIS
is that view, plus the controls to act on it.

Built for the real scale of this deployment: **1.65 M documents**, **10 M chunks /
400 GB** in OpenSearch, **332 connectors**.

---

## Architecture in one paragraph

A Rust workspace (Axum + Tokio + SQLx) serving one HTTP API, with the React UI
embedded in the binary. **The backend is the single data plane**: it is the only
process that holds credentials, and both the UI and the CLI speak to it over HTTP.
Postgres answers list questions (`document.chunk_count` is read directly — the list
path makes zero OpenSearch calls), OpenSearch answers content questions, and every
connector or indexing *action* is proxied to the Onyx API rather than written
behind Onyx's back. Direct database writes are confined to per-document delete and
edit, which Onyx exposes no endpoint for.

| Crate | What it is |
|---|---|
| `ovis-core` | The data plane: Onyx Postgres queries, the OpenSearch client, the Onyx API client, and the wire types shared with the CLI |
| `ovis-backend` | The HTTP/SSE server and the embedded UI |
| `ovis-bench` | The performance acceptance gate |
| `ovis-cli` | `ovis` — CLI and TUI (its own redesign is pending; see `redesign/cli/`) |
| `ovis-prune` | Deduplication and pruning engine (unchanged by this redesign) |

Full design documents live in [`redesign/`](./redesign/); `redesign/backend/` is
what this implementation follows, and
[`redesign/backend/05_AS_BUILT.md`](./redesign/backend/05_AS_BUILT.md) records
where the shipped code deviates from that design and why.

---

## Running it

```bash
cp .env.example .env      # then fill in DATABASE_URL and OPENSEARCH_URL
docker compose up -d --build
curl -fsS localhost:8080/api/v1/system/health | jq
```

Or directly:

```bash
export DATABASE_URL='postgres://postgres:…@192.168.4.113:5433/postgres'
export OPENSEARCH_URL='http://192.168.4.113:9200'
cargo run --release --bin ovis-backend
```

Configuration comes from the environment, optionally layered over a TOML file
(`--config ovis.toml` or `OVIS_CONFIG`), with the environment winning. Every
setting is listed in [`.env.example`](./.env.example). Three things worth
repeating:

- **Connect to Postgres directly (port 5433 here), not pgbouncer (5432).** SQLx
  uses prepared statements; pgbouncer's transaction pooling breaks them. OVIS
  warns at startup if it sees `:5432`.
- **`DATABASE_URL` and `OPENSEARCH_URL` are required.** There is no default and no
  credential compiled into the binary; the server exits with a clear message
  instead of silently connecting somewhere wrong.
- **Set `OVIS_API_TOKEN` before exposing this beyond localhost.** The API includes
  destructive endpoints.

### Before first use: the index migration

```bash
psql "$DATABASE_URL" -f ops/onyx_indexes.sql     # off-peak; uses CONCURRENTLY
```

Eight additive indexes on Onyx's `document` and `document__tag` tables. OVIS never
applies them itself — Onyx owns that schema — but without them the list path
sorts the whole table on every request. Measured on gamma: the default page went
from **965 ms to 0.6 ms**. `GET /api/v1/system/health` lists any that are absent
under `missing_indexes`, as a performance warning rather than an error.

### Onyx API token

Reads work without one. Every *action* — pause, resume, run-once, prune, cc-pair
delete, targeted reindex, boost, hide — answers `503 ONYX_UNCONFIGURED` until
`ONYX_API_URL` and `ONYX_API_KEY` are set.

The redesign called for minting an admin API key via `POST /admin/api-key`. **That
endpoint is paywalled on this deployment**: Onyx v4.3.4 answers

```json
{"error_code":"FEATURE_NOT_AVAILABLE","detail":"This feature requires the Business plan.","required_tier":"business"}
```

before it even looks at credentials, which is why the `api_key` table is empty. The
free-tier equivalent is a **personal access token** (`POST /user/pats`, gated only
on basic access), and it is presented the same way — `Authorization: Bearer …` — so
`ONYX_API_KEY` accepts either.

To mint one:

```bash
scripts/onyx-token.sh          # prompts for the Onyx admin password
```

It logs in, tries `POST /admin/api-key` first (in case the edition ever changes),
falls back to `POST /user/pats`, and prints the `ONYX_API_KEY=…` line to paste into
`.env`. **Onyx returns the raw token exactly once** — there is no way to read it
back, only to revoke it and mint another. The password is read interactively, goes
to curl through a 0600 temp file rather than argv (where `ps` could see it), and is
never written anywhere.

`scripts/onyx-token.sh --list` shows existing tokens; `--revoke <id>` removes one.

The equivalent by hand, if you would rather see it:

```bash
ONYX=http://192.168.4.113:8080
curl -sS -c /tmp/onyx.cookies -X POST "$ONYX/auth/login" \
  --data-urlencode 'username=admin@example.com' \
  --data-urlencode 'password=…'
curl -sS -b /tmp/onyx.cookies -X POST "$ONYX/user/pats" \
  -H 'Content-Type: application/json' \
  -d '{"name":"ovis","expiration_days":null,"scopes":null}'
rm -f /tmp/onyx.cookies
```

Put the returned `token` (it starts `onyx_pat_`) in `ONYX_API_KEY` and restart.
Confirm it took:

```bash
curl -fsS localhost:8080/api/v1/system/health | jq .onyx_api
# -> {"configured": true, "status": "ok", ...}
```

`status: "unauthorized"` means the token was rejected — the health endpoint checks
it against Onyx rather than waiting for the first action to fail.

The same flow is available programmatically as
`ovis_core::onyx::OnyxClient::mint_pat`, which tries the API-key endpoint first and
falls back to a PAT.

---

## The API

`/api/v1`, JSON except for SSE and `…/text`. The complete surface is specified in
[`redesign/backend/03_API_SURFACE.md`](./redesign/backend/03_API_SURFACE.md).

```
GET    /pages                       list: search, connector_id, source, hidden,
                                    chunk_min/max, updated_after/before, sort,
                                    limit + page|cursor
GET    /pages/stream                the same, as SSE
GET    /pages/{id}                  metadata detail
PATCH  /pages/{id}                  semantic_id, boost, hidden, metadata_merge
DELETE /pages/{id}                  cascading delete
POST   /pages/batch-delete          up to OVIS_BATCH_DELETE_MAX
GET    /pages/{id}/chunks           paged, no vectors
GET    /pages/{id}/chunks/{n}/vector   one real stored vector
GET    /pages/{id}/text             text/plain, reconstructed

GET    /search                      q, mode=keyword|semantic|hybrid, …

GET    /connectors                  real status, real doc counts, park state
GET    /connectors/{cc_pair_id}     + config, credential name, attempt aggregates
GET    /connectors/{cc_pair_id}/attempts|errors|docs
POST   /connectors/{cc_pair_id}/pause|resume|run-once|prune
PATCH  /connectors/{cc_pair_id}     name, refresh_freq_secs
DELETE /connectors/{cc_pair_id}     requires {"confirm_name": "<exact name>"}

GET    /indexing/attempts[/{id}]    crawl telemetry, stalled + rate derived
GET    /indexing/background-errors
GET    /indexing/failed-documents
POST   /indexing/targeted-reindex[/{job_id}]

GET    /tags, /tags/keys            facet counts
GET    /stats/overview|index|sources|connectors/top|timeline
GET    /system/health|version|runtime|metrics
```

**Document ids are URLs, and occupy exactly one path segment**, so clients
percent-encode them:

```bash
curl "localhost:8080/api/v1/pages/https%3A%2F%2Fexample.com%2Fa"
```

An unencoded id matches no route and gets a 400 saying so — rather than falling
through to the SPA and returning HTML to a JSON client.

Every non-2xx response carries the same envelope:

```json
{ "error": { "code": "DATABASE", "message": "database error", "status": 500, "req_id": "ms2dmisf0004" } }
```

Clients branch on `code`. `req_id` also arrives as the `x-request-id` header and
appears on every log line for that request, so a client-visible `"database error"`
can be traced to the actual cause without guessing.

### Things the API tells you rather than hiding

- `total_exact: false` — the unfiltered grand total is a planner estimate, because
  an exact `count(*)` over 1.65 M rows takes ~130 ms and would dominate an
  otherwise sub-millisecond response. An exact count lands in the background and
  takes over. Filtered totals are always exact.
- `chunk_count: null` — Onyx has not counted this document yet, which is *not* the
  same as zero. Freshly crawled pages sit here for a while, and `chunk_min`/
  `chunk_max` deliberately exclude them.
- `degraded: "no_knn_field"` — semantic and hybrid search fell back to keyword.
  See "Known limits" below.
- `pg_row: false` — the index holds chunks for a document with no Postgres row
  (orphaned chunks), rather than the response pretending the document is fine.
- `recrawl_risk: true` — the document's connector is still active, so a delete will
  likely be undone at the next scheduled refresh (web `refresh_freq` is 30 days
  here). Durable exclusion is a pruning concern, out of scope for this redesign.
- `index_cleanup_pending: true` — Postgres committed but the index delete failed.
  The id is queued in `ovis.pending_index_deletes` and a background task retries
  it; no silent permanent orphans.
- `parked: true` — the connector's latest attempt carries a resilience-cron
  sentinel (`first-pass already complete` / `park done`). `run-once` on a parked
  connector requires `{"acknowledge_parked": true}` and answers
  `409 PARKED_CONNECTOR` otherwise.
- `stalled: true` — an `IN_PROGRESS` attempt with no heartbeat for 45 minutes.
  Derived from heartbeat staleness, never from document counts: a healthy
  connector can legitimately sit at zero documents for a long time.

---

## Testing

```bash
cargo test --workspace                      # unit + query-shape tests, no services needed

scripts/test-db.sh up                       # throwaway Postgres with the real Onyx DDL
export OVIS_TEST_DATABASE_URL="$(scripts/test-db.sh dsn)"
cargo test --workspace                      # + database and HTTP-contract integration tests

OVIS_SMOKE_URL=http://localhost:8080 \
  cargo test -p ovis-backend --test live_smoke -- --ignored   # read-only, against the real thing

cargo run --release -p ovis-bench -- --url http://localhost:8080   # performance gates
```

The integration tests run against `tests/fixtures/onyx_schema.sql` — a captured
`pg_dump` of the live Onyx schema, including every foreign key the cascading delete
has to clear — so a query that passes them passes in production for the same
reason. Refresh it after an Onyx upgrade with
`scripts/capture-onyx-schema.sh gamma > tests/fixtures/onyx_schema.sql`.

`tests/fixtures/seed.sql` is shaped around the defects being regression-tested: a
tagged document to delete, a document on two connectors, a null `chunk_count`, a
parked connector, a stalled attempt, and timestamps deliberately out of id order.

Database-backed tests skip themselves (loudly) when `OVIS_TEST_DATABASE_URL` is
unset, so `cargo test` works without Docker.

---

## Known limits

Real constraints of this deployment, verified rather than assumed:

**Semantic and hybrid search fall back to keyword.** The live index declares
`embeddings.full_embedding` as a 768-dim `knn_vector` (hnsw/lucene/cosine), but
**zero documents populate it** — Onyx writes its vectors to `content_vector`, typed
as a plain `float` array, which cannot serve kNN or a `script_score` cosine. A kNN
query against it returns zero hits in 1 ms, which would read as "nothing matched".
OVIS probes for a *populated* kNN field at startup and on every runtime refresh; if
there is none, `mode=semantic|hybrid` degrade to BM25 and report
`degraded: "no_knn_field"`. Per-chunk vectors are still served, from
`content_vector`. If a future Onyx re-index populates the kNN field, the probe picks
it up and hybrid search starts working with no code change.

**Connector-scoped search is best-effort.** The chunk index carries no connector
field, so `GET /search?connector_id=…` filters during Postgres hydration over the
top 200–500 global hits. A connector holding a small slice of the corpus may
legitimately show no results for a broad query. The response says
`degraded: "connector_filter_post_applied"` and marks `total_hits_exact: false`
rather than reporting a total that does not match the list.

**The embedded UI has not been migrated yet.** The backend API changed shape, and
`ui/` still reads `total_pages` (gone), `doc_updated_at` (now null for all but
~1,500 of 1.65 M rows — use `updated_at`), and `full_text`/`embeddings` off the
detail response (now separate endpoints). So the browser dashboard at `/` is
currently broken; the API underneath it is not. The frontend is its own track —
see `redesign/frontend/` — and the backend was deliberately given no compatibility
shims, since the plan has the UI and CLI migrating in lockstep.

**Deep offset paging is refused past 50,000 rows** (`400`, pointing at
`next_cursor`). Keyset cursors have no depth limit.

**Connector-filtered *listing* costs more than the unfiltered path.** OVIS picks
between two query shapes from a bounded selectivity probe (≤7 ms); measured on
gamma, the worst case is ~300 ms for the largest connector versus 0.6 ms
unfiltered. The details, with the measurements behind the threshold, are in
`ovis_core::db::documents::CONNECTOR_SELECTIVITY_THRESHOLD`.

---

## Operational notes

- `GET /api/v1/system/health` returns **503 when degraded**, so container and
  reverse-proxy health checks are meaningful. Postgres or OpenSearch being down, or
  the Onyx schema not matching, is degraded; an unconfigured Onyx token or embedder
  costs features, not health.
- SIGTERM drains in-flight requests, bounded by `OVIS_SHUTDOWN_GRACE_SECS`
  (default 10). Verified: a 4,000-row SSE stream in flight completed in full and
  the process exited 0.1 s later.
- The OpenSearch index name is read from `search_settings WHERE status='PRESENT'`
  every 60 s and never hardcoded, so an Onyx re-embed switchover retargets
  automatically. The `danswer_chunk*` wildcard is never used — during a re-embed it
  would span two indexes.
- The startup probe verifies every column OVIS reads and every restricting foreign
  key onto `document(id)`. A drift becomes `501 SCHEMA_MISMATCH` on the affected
  endpoint and appears in `/system/health`, rather than a wrong answer or a
  transaction that fails halfway through a delete.
- `GET /api/v1/system/metrics` exposes Prometheus text: request histograms, pool
  size and idle count, cache entry counts, pending index deletes, and gauges for
  `ovis_pg_up`, `ovis_schema_ok` and `ovis_knn_ready`.
- **Deleted documents can come back.** A document whose connector is still ACTIVE
  will be re-crawled at the next refresh. Every delete surface reports
  `recrawl_risk`.

### Outstanding chore, independent of OVIS

The Postgres password is committed in the proxmox-setup worker compose files, and
was baked into pre-redesign OVIS binaries. It is worth rotating. Nothing in this
repository contains it any more:

```bash
grep -rnE '192\.168\.|postgres://[^ ]*:[^ @]*@' --include='*.rs' --include='*.toml' crates/
```
