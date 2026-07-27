# Troubleshooting

Symptom → cause → fix, for everything we have actually seen. Start with:

```bash
curl -sS localhost:8080/api/v1/system/health | jq     # or: ovis status
```

Every error response carries `error.code` and a `req_id` that matches the
server's log lines for that request — grep the log for the `req_id` before
guessing.

## Server won't start

**"configuration error: DATABASE_URL is required"** (or OPENSEARCH_URL) — the
required settings have no defaults by design. Fill `.env` and make sure it is
actually exported (`set -a; . ./.env; set +a`) or passed to compose.

**Warning about port 5432 / prepared-statement errors** — you pointed
`DATABASE_URL` at pgbouncer. Connect to Postgres directly (5433 on the
reference deployment); pgbouncer's transaction pooling breaks SQLx's prepared
statements.

**Port already in use** — something else owns 8080. `lsof -ti:8080
-sTCP:LISTEN` to find it; `ovis server status` will tell you whether it is an
OVIS (and healthy) or a foreign process rather than false-positive on any
listener.

**`/` says "UI assets are not embedded in this build"** — the binary was
compiled before `ui/dist` existed. `cd ui && npm install && npm run build`,
then rebuild the backend. The API works regardless.

## Health says degraded

- `postgres: down` / `opensearch: down` — connectivity/credentials to those
  stores; OVIS retries continuously, nothing to restart once they return.
- `schema_ok: false` with `missing_columns` — an Onyx upgrade changed a table
  OVIS reads. Affected endpoints answer `501 SCHEMA_MISMATCH` instead of
  guessing. Update OVIS (or file the drift) rather than ignoring it.
- `missing_indexes: [...]` — *not* degraded, just slow: apply
  `ops/onyx_indexes.sql` off-peak.

## Search behaves oddly

**`degraded: "no_knn_field"` on semantic/hybrid** — the chunk index declares a
kNN field that no document populates (seen mid Vespa→OpenSearch migrations).
OVIS probes for a *populated* field every 60 s; results are BM25 keyword until
a re-embed fills it, at which point vector modes start working with no
restart. Not an error — the response tells you what actually ran.

**`degraded: "no_embedder"`** — `EMBED_API_URL` unset or unreachable. Set it
(and `EMBED_MODEL`) to enable vector modes.

**`degraded: "connector_filter_post_applied"` / missing results with
`connector_id`** — the chunk index carries no connector field, so the scope is
applied after ranking over the top global hits. A connector holding a small
slice of the corpus can legitimately return nothing for a broad query; the
list endpoints are the authoritative per-connector view.

**A freshly crawled page doesn't match `chunk_min=1`** — its `chunk_count` is
still null ("not counted yet"), and the chunk filters deliberately exclude
nulls. It appears once Onyx records the count.

## Actions fail

**`503 ONYX_UNCONFIGURED`** — set `ONYX_API_URL` + `ONYX_API_KEY`
([getting started](./getting-started.md#the-onyx-api-token)).

**`402 FEATURE_NOT_AVAILABLE` while minting a key** — you called Onyx's
`POST /admin/api-key` on a free-tier edition; it is paywalled before it reads
credentials. Use a personal access token instead — `ovis server
setup-onyx-key` and `scripts/onyx-token.sh` both do the fallback for you.

**`onyx_api.status: "unauthorized"` in health** — the token was rejected
(revoked, or from a different Onyx). Mint a new one; Onyx cannot re-show an
existing token.

**`409 PARKED_CONNECTOR` on run-once** — the resilience cron finished this
cc-pair on purpose (its last attempt carries a park sentinel). Both shipped
clients show an explainer and ask; overriding requires
`"acknowledge_parked": true`, which nothing sets automatically.

**Connector delete refused** — the body's `confirm_name` must match the
cc-pair name exactly. That is the guard, not a bug.

**`400` with `UNKNOWN_PARAM`** — a typoed query parameter (`search_mode`
instead of `mode` is the classic). The message names the valid set.

**`400` on `/pages/<id>`** — the document id wasn't percent-encoded into a
single path segment. Ids are URLs; encode them
(`/pages/https%3A%2F%2Fexample.com%2Fa`).

## Crawling looks stuck

**An attempt sits `NOT_STARTED`** — normal queueing behind other in-flight
attempts; the reference deployment has run 25-minute queues. Not a stall.

**`stalled: true`** — IN_PROGRESS with no heartbeat for 45 minutes, usually a
zombie left by a worker restart; it blocks its cc-pair until cancelled.
The reference deployment clears zombies with a cron
(`unstick_zombies`); after clearing, expect a document-output pause while the
crawl restarts from its sitemap — liveness is judged on heartbeats, never on
document counts.

**A healthy connector sits at 0 documents** — legitimate, sometimes for a long
time (sitemap walking, first-pass policies). See previous item.

**`read_only: true` on the index / indexing halted** — OpenSearch tripped its
flood-stage disk watermark and set the index read-only. Free disk, clear the
block, watch the Stats disk gauge afterwards.

## Client-side oddities

| Symptom | Meaning |
|---|---|
| CLI exit 12 | server unreachable — wrong `--server`/profile, or it's down |
| CLI exit 13 | the server answered but reports degraded |
| CLI exit 14 | your `@N` handle is older than an hour — re-run the list |
| CLI exit 11 + a stream warning | `--all` hit the server's `OVIS_MAX_STREAM_LIMIT`; page through or raise the cap |
| UI shows "live unavailable — polling" | the SSE stream errored; the list falls back to normal fetching |
| UI banner "stream ended at N of M" | the live stream hit the server cap; switch live off for the full set via paging |
| `~` in front of a total | planner estimate (`total_exact: false`); an exact count takes over in the background |
| Totals differ between the UI footer and a moment ago | the crawlers are writing; both numbers were true when served |

## Still stuck

Grep the server log for the response's `req_id`, then open an issue with the
log lines, the request, and `ovis status -o json` output.
