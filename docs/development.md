# Development

## Repository layout

```
crates/
├── ovis-core/       data plane: db queries, OpenSearch + Onyx clients, api_types (the wire contract)
├── ovis-backend/    Axum server, SSE, background tasks, embedded UI (assets.rs)
├── ovis-cli/        the `ovis` binary — CLI + TUI, pure API client
├── ovis-prune/      detection: quality gates, MinHash dedup, URL canonicalisation (no I/O)
├── ovis-llm/        provider-agnostic model access + the capability probe
└── ovis-bench/      performance acceptance gates
ui/                  React 19 + TS + Tailwind v4 (see ui/README.md)
docs/                this documentation
ops/                 onyx_indexes.sql — the Postgres support indexes
scripts/             test-db.sh · onyx-token.sh · capture-onyx-schema.sh
tests/fixtures/      captured Onyx schema + regression-shaped seed data
```

The wire types in `ovis-core::api_types` are the contract: the server
serialises them, the CLI compiles against them, and `ui/src/api/types.ts`
mirrors them field-for-field (nullability included — several nulls are
load-bearing).

## Building

```bash
(cd ui && npm install && npm run build)   # optional; backend compiles without it
cargo build --release                     # ovis-backend + ovis
```

`ovis-backend/build.rs` creates `ui/dist` if absent (rust-embed needs the
folder), stamps git/rustc/build-time metadata, and rebuilds when `ui/dist`
changes. Docker builds run the UI stage automatically.

## Testing

```bash
cargo test --workspace                    # unit + query-shape tests; no services needed
```

Database-backed tests activate when a throwaway Postgres is up:

```bash
scripts/test-db.sh up
export OVIS_TEST_DATABASE_URL="$(scripts/test-db.sh dsn)"
cargo test --workspace                    # + DB and HTTP-contract integration tests
```

They run against `tests/fixtures/onyx_schema.sql` — a captured `pg_dump` of a
real Onyx schema including every foreign key the cascading delete must clear —
so a query that passes them passes in production for the same reason. Refresh
after an Onyx upgrade:

```bash
scripts/capture-onyx-schema.sh <host> > tests/fixtures/onyx_schema.sql
```

`tests/fixtures/seed.sql` is shaped around regression cases: a tagged document
to delete, a document on two connectors, a null `chunk_count`, a parked
connector, a stalled attempt, timestamps deliberately out of id order.
`tests/fixtures/seed_prune.sql` adds the pruning shapes on top — exact-dup
groups, aged stubs on paused *and* active pairs, a document hidden before
staging, a German page, a near-duplicate pair — and is applied only by
`crates/ovis-backend/tests/prune_api.rs`, which is where every hard-delete
path runs for real (never against a live deployment).
Tests skip themselves *loudly* when the DSN is unset, so plain `cargo test`
works without Docker.

Read-only smoke against a live deployment, and the performance gates:

```bash
OVIS_SMOKE_URL=http://localhost:8080 cargo test -p ovis-backend --test live_smoke -- --ignored
cargo run --release -p ovis-bench -- --url http://localhost:8080
```

## UI development

```bash
cd ui
npm run dev          # Vite on :3000 (falls back to :3001), /api proxied to :8080
npm run typecheck && npm run lint && npm run lint:tokens && npm test
node scripts/axe-audit.mjs                          # WCAG audit, non-zero on violations
node scripts/drive.mjs http://localhost:3000/pages shot 1440 900   # headless screenshots
```

Run the backend first — the UI is developed against live data on principle;
the bugs that matter (mis-encoded ids, nulls rendered as zero, silently
dropped filters) only show up against real data. Conventions that are
enforced: colors come only from `theme.css` tokens (the default Tailwind
palette is deleted; `lint:tokens` catches the rest), one component tree across
viewports, every mutation optimistic-with-rollback, and `/lab` renders every
primitive in both viewport classes.

## Quality gates

```bash
cargo clippy --workspace --all-targets    # clean: zero warnings across the workspace
cargo deny check                          # advisories, bans, licenses, sources
cargo audit                               # allowed exceptions documented in .cargo/audit.toml
```

## Continuous integration

`.github/workflows/ci.yml` runs on every push to `main`, every version tag and
every pull request. Four jobs; the first three run in parallel and the fourth
waits on all of them:

| Job | What it runs |
|---|---|
| `rust` | `cargo clippy --workspace --all-targets -- -D warnings`, then `cargo test --workspace` against a Postgres service container loaded with the captured Onyx schema, `seed.sql` and the support indexes |
| `ui` | `npm ci`, typecheck, eslint, the design-token check, vitest, `vite build` |
| `supply chain` | `cargo deny check advisories bans licenses sources` |
| `publish image` | the GHCR build — **only** after the other three pass, and never for a pull request |

Two things about it are worth knowing.

**The image cannot be published from a red commit.** The publish step is a job
with `needs`, not a separate workflow. `:latest` is what the homelab's
watchtower pulls, so a failing test has to be able to stop it.

**CI sets `OVIS_REQUIRE_TEST_DATABASE=1`.** The DB-backed suites skip
themselves when `OVIS_TEST_DATABASE_URL` is unset — right for a laptop without
Docker, and dangerous in CI, because cargo captures a passing test's stderr:
a run with no database reports every test `ok` in 0.00 s and prints nothing at
all. That variable turns the skip into a failure, so a service container that
never came up fails the run instead of producing a green tick that proves
nothing. Set it locally too if you want the same insurance.

`cargo fmt` is deliberately **not** a gate: the tree is not currently
rustfmt-clean (224 hunks across 44 files, much of it hand-aligned SQL), and
adopting it is a reformatting commit someone should take on purpose rather
than a surprise from CI.

Dependency updates arrive as grouped weekly Dependabot PRs, which is only safe
because those PRs run the pipeline above before anyone reads them.

## Ground rules

The project's non-negotiables, which patches should keep:

- **No fabricated data, ever.** No sample fallbacks, no fake success, no
  invented numbers. A failure is an error the user can see.
- **Honest fields flow through.** If the API states uncertainty
  (`total_exact`, `chunk_count: null`, `degraded`, …), surfaces render it.
- **Landmines.** Connect to Postgres directly, never through pgbouncer. Never
  clobber the resilience-cron park sentinels in `index_attempt.error_msg`. No
  blanket crawl triggers — crawl kicks go one cc-pair at a time. Derive doc
  counts from `document_by_connector_credential_pair`, never from Onyx's
  `total_docs_indexed`. Judge liveness by heartbeat staleness, never by
  document counts. Discover the index name from `search_settings` — never
  hardcode it, and never use the `danswer_chunk*` wildcard.
- When reality contradicts documentation, do what is right and **update the
  docs in the same change** — every behavioural claim in `docs/` is meant to
  stay true.
