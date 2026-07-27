# `ovis-backend`

The OVIS HTTP API, and the only process in the system that holds credentials.

**The backend is the single data plane.** The CLI and the UI both speak this
API over HTTP; neither has a database password. The architecture is documented
in [`docs/architecture.md`](../../docs/architecture.md).

## Running it

```bash
export DATABASE_URL='postgres://postgres:…@192.168.4.113:5433/postgres'
export OPENSEARCH_URL='http://192.168.4.113:9200'
cargo run --release --bin ovis-backend
```

`DATABASE_URL` and `OPENSEARCH_URL` are required and have no defaults — the
server exits with a clear message rather than silently connecting somewhere
wrong. Configuration is environment-first, optionally layered over a TOML file
(`--config ovis.toml` or `OVIS_CONFIG`), with the environment winning. Every
setting is in [`.env.example`](../../.env.example).

`ovis server start` runs this same server from the CLI binary; see
[`../ovis-cli/README.md`](../ovis-cli/README.md).

**Connect to Postgres directly (5433 here), not pgbouncer (5432).** SQLx uses
prepared statements and pgbouncer's transaction pooling breaks them. The server
warns at startup if it sees `:5432`.

## Layout

```
src/
├── lib.rs          app() · build_state() · serve_with_shutdown() · spawn_background_tasks()
├── config.rs       ServerConfig: env + TOML, validation, redacted summary
├── state.rs        AppState: pools, caches, runtime metadata, index capabilities
├── error.rs        AppError -> status + stable code + req_id envelope
├── extract.rs      Query/Json extractors that reject unknown fields
├── assets.rs       the embedded UI (rust-embed) and the SPA fallback
├── middleware/     request id, auth, timeout, metrics, error rendering
├── routes/         parse and map only — no SQL, no OpenSearch JSON
└── services/       how an answer is assembled
```

The data plane itself lives in [`ovis-core`](../ovis-core): `db` for Postgres,
`search` for OpenSearch and the embedder, `onyx` for the Onyx API. A route
handler carries no query text.

## The shape of the API

`/api/v1`, JSON except for SSE and `…/text`. The full surface is documented in
[`docs/api.md`](../../docs/api.md).

Three properties worth knowing before you call it:

- **Document ids are URLs and occupy exactly one path segment**, so clients
  percent-encode them. An unencoded id matches no route and gets a `400` saying
  so, rather than falling through to the SPA and returning HTML to a JSON client.
- **Every non-2xx carries the same envelope** — `{error: {code, message, status,
  req_id}}`. Clients branch on `code`. `req_id` is also the `x-request-id` header
  and appears on every log line for that request, so a client-visible "database
  error" can be traced to its actual cause.
- **A failure is never a 200.** `GET /system/health` returns **503** when
  degraded, so container and reverse-proxy health checks mean something.

## Honest fields

The API tells you things rather than hiding them. Clients are expected to render
these, not smooth them over:

| Field | Means |
|---|---|
| `total_exact: false` | the unfiltered grand total is a planner estimate; filtered totals are always exact |
| `chunk_count: null` | Onyx has not counted this document yet — **not** the same as `0` |
| `degraded` | the search ran, but not the way you asked; an open string (`no_knn_field`, `no_embedder`, `connector_filter_post_applied`) |
| `pg_row: false` | the index holds chunks for a document with no Postgres row |
| `recrawl_risk: true` | the connector is still active, so a delete will likely be undone |
| `index_cleanup_pending` | Postgres committed but the index delete did not; the id is queued and retried |
| `parked: true` | the resilience cron finished with this cc-pair on purpose |
| `stalled: true` | an `IN_PROGRESS` attempt with no heartbeat for 45 minutes |

## Tests

```bash
cargo test -p ovis-backend                                  # no services needed

scripts/test-db.sh up                                       # throwaway Postgres, real Onyx DDL
export OVIS_TEST_DATABASE_URL="$(scripts/test-db.sh dsn)"
cargo test -p ovis-backend                                  # + database and HTTP-contract tests

OVIS_SMOKE_URL=http://localhost:8080 \
  cargo test -p ovis-backend --test live_smoke -- --ignored  # read-only, against the real thing
```

Integration tests run against `tests/fixtures/onyx_schema.sql`, a captured
`pg_dump` of the live Onyx schema including every foreign key the cascading
delete has to clear — so a query that passes them passes in production for the
same reason. Database-backed tests skip themselves loudly when
`OVIS_TEST_DATABASE_URL` is unset.

## Performance

`cargo run --release -p ovis-bench -- --url http://localhost:8080` is the
acceptance gate: p50 list < 15 ms and p99 < 60 ms at 1.67 M documents, search
p99 < 150 ms, first SSE byte < 30 ms. The default list page measures 10.5 ms p50.

That depends on eight additive indexes on Onyx's tables:

```bash
psql "$DATABASE_URL" -f ops/onyx_indexes.sql     # off-peak; uses CONCURRENTLY
```

OVIS never applies them itself — Onyx owns that schema — but without them the
list path sorts the whole table on every request (965 ms → 0.6 ms, measured).
`GET /api/v1/system/health` lists any that are absent under `missing_indexes`, as
a performance warning rather than an error.
