# Operations

Day-2 concerns: deploying, watching, securing, and understanding OVIS in
production.

## Deployment

**Docker** (recommended): `docker compose up -d --build`. The image runs
unprivileged, honours SIGTERM, and health-checks against
`/api/v1/system/health` — which answers **503 when degraded**, so
orchestrator checks are meaningful. `OVIS_BIND` controls which interface
compose publishes on (default localhost).

**Single binary**: build with the UI embedded and copy one file. The CLI can
supervise it (`ovis server start -d` / `status` / `stop`) with a PID file,
health-gated startup, and logs in `$XDG_STATE_HOME/ovis/server.log`.

**Placement.** Every performance figure in this documentation was measured
across a LAN, which adds ~7 ms per Postgres round trip. Running OVIS on the
same host as Postgres removes that; nothing else changes.

## Configuration reference

[`.env.example`](../.env.example) is the annotated, authoritative list. The
operational ones:

| Variable | Default | Notes |
|---|---|---|
| `OVIS_HOST` / `OVIS_PORT` | `127.0.0.1` / `8080` | direct-run bind; `OVIS_BIND` is compose-only |
| `OVIS_API_TOKEN` | unset | bearer auth; **set before exposing beyond localhost** |
| `OVIS_CORS_ORIGINS` | `*` | tighten alongside the token |
| `OVIS_DB_MAX_CONNECTIONS` | 20 | OVIS needs no pooler |
| `OVIS_MAX_PAGE_SIZE` | 500 | list `limit` ceiling |
| `OVIS_MAX_STREAM_LIMIT` | 10000 | SSE stream ceiling — clients detect and report the cap |
| `OVIS_BATCH_DELETE_MAX` | 1000 | batch-delete size ceiling |
| `OVIS_REQUEST_TIMEOUT_SECS` | 30 | per-request budget → `504 TIMEOUT` |
| `OVIS_SHUTDOWN_GRACE_SECS` | 10 | drain window on SIGTERM |
| `OVIS_RUNTIME_REFRESH_SECS` | 60 | index-name / kNN-capability re-probe |
| `OVIS_LOG_FORMAT` / `RUST_LOG` | text / info | `json` for shippers |

A blank optional value (`OVIS_API_TOKEN=` sourced with `set -a`) normalises to
*unset* — it never becomes an empty token that any caller could satisfy.

## Health

`GET /api/v1/system/health` reports each dependency with latency, plus schema
verification:

- **Degraded (HTTP 503):** Postgres or OpenSearch down, or the Onyx schema not
  matching what OVIS reads.
- **Not degraded:** a missing Onyx token or embedder — those cost features
  (actions, vector modes), not health, and show as `configured: false` /
  degraded search instead.
- `missing_indexes` lists any OVIS support indexes absent from Postgres — a
  performance warning, never an error.
- `missing_columns` / `unhandled_document_fk_children` report schema drift;
  affected endpoints answer `501 SCHEMA_MISMATCH` rather than risking a wrong
  answer or a half-completed cascade.

The CLI's `ovis status` renders the same data and exits 13 when degraded.

## Metrics

`GET /api/v1/system/metrics` (Prometheus text): request histograms by
route/status, DB pool size and idle count, cache entry counts, pending index
deletes, and gauges for `ovis_pg_up`, `ovis_schema_ok`, `ovis_knn_ready`.
Useful alerts: `ovis_pg_up == 0`, `ovis_schema_ok == 0`, pending index deletes
growing, and the disk figures from `/stats/overview` (below).

## Performance

Budgets are enforced by `ovis-bench` (10/10 gates passing at 1.65 M documents,
measured over a LAN):

| Path | Budget | Measured |
|---|---|---|
| Default list page, p50 | < 15 ms | **10.5 ms** |
| List p99 | < 60 ms | passes |
| Content search p99 | < 150 ms | passes |
| SSE first byte | < 30 ms | **0.8 ms** |

```bash
cargo run --release -p ovis-bench -- --url http://localhost:8080
```

Two paths cost more by nature, and say so in `docs/api.md`: connector-filtered
listing (bounded selectivity probe; worst case ~300 ms on a 105k-doc
connector) and connector-scoped content search (post-ranking filter).

Keep the **index migration** applied (`ops/onyx_indexes.sql`); without it the
list path is ~965 ms instead of sub-millisecond. Re-check via
`missing_indexes` in health after Onyx upgrades.

## Security

- **Credentials live only in the backend's environment.** The CLI and UI never
  hold DB/OpenSearch credentials; nothing is compiled into any binary; `ps`
  never shows a secret (the CLI passes credentials via environment and 0600
  files, never argv).
- **Set `OVIS_API_TOKEN` the moment OVIS is reachable beyond localhost** — the
  API includes destructive endpoints (document delete, connector delete).
  Health stays open for probes; everything else requires the bearer.
- The Onyx token is minted once, stored at mode 0600 (CLI config) or in
  `.env`; Onyx cannot re-show it, only revoke and re-mint
  (`scripts/onyx-token.sh --list | --revoke <id>`).
- The gitignore is deny-by-default for secret-shaped filenames; `.env` is
  never committed.

## Behaviour worth knowing before it surprises you

- **Deleted documents can come back.** A document whose connector is still
  ACTIVE will be re-crawled at its next refresh (30 days for web connectors on
  the reference deployment). Every delete surface reports `recrawl_risk`;
  durable exclusion is a pruning concern, out of scope here.
- **Parked connectors are finished on purpose.** The resilience cron marks
  first-pass-complete cc-pairs with a sentinel in their last attempt error;
  OVIS surfaces this as `parked` and gates `run-once` behind an explicit
  acknowledgement. Un-parking restarts a crawl from its sitemap.
- **`stalled` means no heartbeat for 45 minutes** — the same heuristic the
  resilience cron uses. Zombie attempts (workers restarted mid-crawl) block
  their cc-pairs until cleared; the reference deployment clears them with a
  cron.
- **OpenSearch disk is a first-class concern.** The reference index has
  previously tripped the flood-stage watermark, which sets the index
  read-only. `/stats/overview` carries `disk_used_pct` and `read_only`; the
  Stats view shows a gauge with an alarm state. If `read_only` is true,
  indexing is halted until disk is freed and the block cleared.
- **Index switchover is automatic.** The chunk index name is re-read from
  `search_settings` every 60 s, so an Onyx re-embed retargets OVIS without a
  restart — and if the new index has a *populated* kNN field, semantic/hybrid
  search starts working with no code change.
- **Graceful shutdown is real.** SIGTERM drains in-flight requests (bounded by
  the grace period); a multi-thousand-row SSE stream in flight completes
  before exit.
- **Failed index deletes retry themselves.** `ovis.pending_index_deletes`
  (the only schema OVIS creates) queues them; watch its gauge in metrics.

## Upgrades

1. Rebuild (`docker compose up -d --build`, or `npm run build` + `cargo build
   --release`). UI and API ship together in one artifact, so they cannot skew.
2. After an **Onyx** upgrade: check `/system/health` for schema drift and
   `missing_indexes`; refresh the test fixture if developing
   (`scripts/capture-onyx-schema.sh <host> > tests/fixtures/onyx_schema.sql`).
3. The Postgres password on the reference deployment was rotated 2026-07-26;
   the procedure and fallout notes live in
   [`redesign/backend/05_AS_BUILT.md`](../redesign/backend/05_AS_BUILT.md) §5.
