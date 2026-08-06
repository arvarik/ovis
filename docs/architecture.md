# Architecture

OVIS is a Rust workspace serving one HTTP API, with a React UI embedded in the
binary and a CLI that speaks the same API. It was rebuilt in 2026 from a
verified audit of its predecessor, and every design decision below was checked
against a live deployment rather than assumed.

## One data plane

```
                    ┌────────────────────────────────────────────┐
   browser ────────▶│                ovis-backend                │
   (embedded UI)    │                                            │
                    │  Postgres ◀── lists, counts, detail        │──▶ Onyx Postgres (direct, :5433)
   ovis CLI ───────▶│  OpenSearch ◀─ content search, chunks      │──▶ OpenSearch (chunk index)
   (HTTP only)      │  Onyx API ◀── every connector/index action │──▶ Onyx API (bearer token)
                    │  Embedder ◀── optional, semantic/hybrid    │──▶ vLLM /v1/embeddings
                    └────────────────────────────────────────────┘
```

**The backend is the only process holding credentials.** The CLI and UI are
pure API clients — the CLI compiles against the same wire structs the server
serialises, so a shape change is a compile error, not a runtime surprise.

Responsibilities are split by what each store is good at:

- **Postgres answers list questions.** Filter/sort/count run entirely on
  Postgres — `document.chunk_count` is read as a column, so the list path makes
  **zero OpenSearch calls**. Pagination is keyset (opaque `next_cursor`) with
  no depth limit; offset paging is allowed but refused past 50,000 rows.
- **OpenSearch answers content questions.** BM25 keyword search with
  highlighting, per-document chunk listings by deterministic id
  (`{document_id}__{chunk_index}`), and real stored vectors. Semantic/hybrid
  modes are wired and self-detecting: a startup + 60 s runtime probe looks for
  a *populated* kNN field, and until one exists those modes degrade to keyword
  and say so (`degraded: "no_knn_field"`).
- **The Onyx API performs actions.** Pause, resume, run-once, prune, rename,
  cc-pair delete, targeted reindex, boost, hide — all proxied through Onyx with
  a token, never written behind Onyx's back. The one exception is per-document
  delete/edit, which Onyx exposes no endpoint for: OVIS does those directly
  with complete foreign-key coverage, and queues index cleanup for retry when
  OpenSearch can't confirm (`ovis.pending_index_deletes`, the only DDL OVIS
  owns).

## The workspace

| Crate | What it is |
|---|---|
| `ovis-core` | The data plane: Postgres queries, the OpenSearch client, the Onyx API client, and `api_types` — the wire structs shared with the CLI |
| `ovis-backend` | Axum HTTP/SSE server, background tasks, and the embedded UI (`rust-embed`) |
| `ovis-cli` | `ovis` — CLI + ratatui TUI; an API client holding no credentials |
| `ovis-prune` | The detection engine: published text-quality gates, MinHash/LSH near-duplicate signatures, URL canonicalisation and asset classification. Pure functions over text and URLs — no I/O, no database |
| `ovis-llm` | Provider-agnostic model access (llama.cpp, Ollama, OpenAI-compatible, Gemini, Anthropic) and the capability probe that decides whether a model may be given work |
| `ovis-bench` | The performance acceptance gate |
| `ui/` | React 19 + TypeScript + Tailwind v4, compiled by Vite, embedded at build time |

`ovis-prune` is deliberately I/O-free: it decides *what a piece of text or a
URL is*, and the backend decides what to do about it. That is what lets the
detection thresholds be unit-tested against fixtures and swept offline
(`cargo run -p ovis-prune --example gate_sweep`) without a database in sight.

## Design principles (the ones that shape behaviour)

These are the project's pillars, and they are enforced, not aspirational:

1. **Honest failures.** A database error is a 5xx with a stable machine code —
   never an empty 200. No surface fabricates data; there are no sample-data
   fallbacks anywhere. Every mutation reports what actually happened.
2. **Honest fields.** The API says what it knows and what it doesn't:
   `chunk_count: null` is "not counted yet" (which is *not* zero),
   `total_exact: false` marks a planner estimate, `pg_row: false` marks
   orphaned chunks, `recrawl_risk` warns that a delete will likely be undone,
   `parked` and `stalled` carry operational truth. Clients render these as
   what they mean. The full list is in the [API guide](./api.md#the-honest-fields).
3. **Fast by architecture.** p50 list < 15 ms at 1.65 M documents (measured
   10.5 ms over a LAN); search p99 < 150 ms; SSE first byte < 30 ms. The
   budgets are enforced by `ovis-bench`, not asserted in prose.
4. **Guardrails match the blast radius.** Connector delete requires the exact
   name typed back. Run-once on a parked connector requires explicit
   acknowledgement. There is no bulk crawl trigger anywhere, by grammar.
5. **Secrets stay out of source.** No DSN, host, or password is compiled into
   any binary; configuration is environment-first with no dangerous defaults.

## How a request flows

- `GET /pages` → one Postgres round trip (keyset page + filtered count), items
  straight from columns. A connector/source filter runs a bounded selectivity
  probe (≤ 7 ms) to choose between two query plans — worst case ~300 ms on the
  largest connector versus 0.6 ms unfiltered.
- `GET /search?q=…` → OpenSearch query (collapsed to one hit per document,
  highlighted), then a single `ANY($1)` Postgres hydration for metadata.
  Connector-scoped search filters during hydration and marks itself
  `degraded: "connector_filter_post_applied"` with `total_hits_exact: false`.
- `GET /pages/stream` → Server-Sent Events in keyset batches of 200, so a slow
  client never pins a Postgres connection; heartbeat comments every 15 s; a
  terminal `done` event carries `total_matched` so clients can detect the
  server's stream cap instead of mistaking it for the whole set.
- `POST /connectors/{id}/run-once` → proxied to Onyx, audit-logged, with the
  parked-sentinel check in front (`409 PARKED_CONNECTOR` without an explicit
  acknowledgement).
- `GET /` → the embedded UI. Hashed assets get immutable one-year caching;
  `index.html` is never cached; a missing *asset* is a 404 while a missing
  *route* falls back to the SPA shell (document ids are URLs, so route
  segments full of dots must not be mistaken for files).

## Schema stewardship

OVIS reads Onyx's schema but does not own it. At startup (and in
`/system/health`) it verifies every column it reads and every restricting
foreign key onto `document(id)`; drift becomes `501 SCHEMA_MISMATCH` on the
affected endpoints rather than a wrong answer. The OpenSearch index name is
discovered from `search_settings WHERE status='PRESENT'` every 60 s — never
hardcoded, and never the `danswer_chunk*` wildcard (during a re-embed that
would span two indexes).

## Reference deployment

The system OVIS was built and measured against — useful as a scale reference:

| Node | Role |
|---|---|
| gamma | Postgres 15 (direct :5433, pgbouncer :5432), OpenSearch (:9200), Onyx API (:8080) |
| omega, zeta | Celery crawl/index workers |
| hppc | 4× vLLM `snowflake-arctic-embed-m-v1.5` behind an nginx LB (:8090) |
| infra | Connector proxy (:8765, CF bypass + sitemap synthesis), ingress |

Scale: **1.67 M documents · 10.1 M chunks / ~375 GB · 332 connectors**.
Everything in OVIS is designed for ~2 M documents, not the hundred-row happy
path.
