# Getting started

OVIS sits *beside* an existing [Onyx](https://github.com/onyx-dot-app/onyx)
(formerly Danswer) deployment. It needs read access to Onyx's Postgres and
OpenSearch, and — for connector actions — a token for the Onyx API. It never
runs its own crawl and owns almost none of its own state.

## Prerequisites

- **An Onyx deployment** with Postgres and OpenSearch reachable from wherever
  OVIS runs. Direct Postgres access is required (see the pgbouncer warning
  below).
- **Docker**, or to build from source: **Rust** (stable) and **Node 20.19+ /
  22+** for the web UI.

## Quick start (Docker)

```bash
git clone <this-repo> ovis && cd ovis
cp .env.example .env      # fill in DATABASE_URL and OPENSEARCH_URL
docker compose up -d --build
curl -fsS localhost:8080/api/v1/system/health | jq
```

The image is a multi-stage build: the UI compiles first (Node stage), then the
Rust binary embeds it — the runtime image is a single executable plus CA
certificates, running unprivileged, with a health check that actually means
something (503 when a dependency is down).

## Quick start (from source)

```bash
git clone <this-repo> ovis && cd ovis
(cd ui && npm install && npm run build)   # the UI embeds into the binary
cp .env.example .env                      # fill in the required values
set -a; . ./.env; set +a
cargo run --release --bin ovis-backend
```

Skipping the UI build is fine — the API is complete without it and `/` answers
"UI assets are not embedded in this build" until you run it.

The CLI comes from the same workspace:

```bash
cargo build --release -p ovis-cli
./target/release/ovis status             # or: cargo install --path crates/ovis-cli
```

The `ovis` binary can also host the backend itself (`ovis server start -d`) —
one file to copy to a homelab box. See the [CLI guide](./cli.md#hosting-the-backend).

## Configuration

Everything comes from the environment, optionally layered over a TOML file
(`--config ovis.toml` or `OVIS_CONFIG`), with the environment winning.
[`.env.example`](../.env.example) documents every setting; the essentials:

| Setting | Required | Notes |
|---|---|---|
| `DATABASE_URL` | **yes** | Onyx's Postgres, connected **directly** — not through pgbouncer |
| `OPENSEARCH_URL` | **yes** | Onyx's OpenSearch |
| `ONYX_API_URL` / `ONYX_API_KEY` | for actions | without them every action answers `503 ONYX_UNCONFIGURED`; reads work |
| `EMBED_API_URL` / `EMBED_MODEL` | for semantic search | without them `mode=semantic\|hybrid` degrade to keyword and say so |
| `OVIS_API_TOKEN` | before exposing | bearer auth on every route; the API includes destructive endpoints |

There are **no defaults for the required values** and no credential compiled
into the binary: the server exits with a clear message rather than silently
connecting somewhere wrong.

> **pgbouncer warning.** SQLx uses prepared statements; pgbouncer's transaction
> pooling breaks them. Connect to Postgres directly (port 5433 on the reference
> deployment, not 5432). OVIS warns at startup if it sees `:5432`, and its own
> pool is ≤ 20 connections — it has no need for a pooler.

## Before first use: the index migration

```bash
psql "$DATABASE_URL" -f ops/onyx_indexes.sql     # off-peak; uses CONCURRENTLY
```

Eight additive indexes on Onyx's `document` and `document__tag` tables. OVIS
never applies them itself — Onyx owns that schema — but without them the list
path sorts the whole table on every request. Measured at 1.65 M documents: the
default page went from **965 ms to 0.6 ms**. `GET /api/v1/system/health` lists
any that are absent under `missing_indexes`, as a performance warning rather
than an error.

## The Onyx API token

Reads work without one. Every *action* — pause, resume, run-once, prune,
cc-pair delete, targeted reindex, boost, hide — needs `ONYX_API_URL` and
`ONYX_API_KEY`.

The obvious route (`POST /admin/api-key`) is **paywalled on free-tier Onyx** —
v4.3.4 answers `402 FEATURE_NOT_AVAILABLE` before it even looks at
credentials. The working equivalent is a **personal access token**
(`POST /user/pats`), presented identically as `Authorization: Bearer …`, so
`ONYX_API_KEY` accepts either. Two helpers mint one:

```bash
ovis server setup-onyx-key     # prompts for the Onyx password, stores the token
scripts/onyx-token.sh          # shell equivalent; prints ONYX_API_KEY=… for .env
```

Both log in, try the API-key endpoint first (in case the edition ever changes),
and fall back to a PAT. **Onyx returns the raw token exactly once** — there is
no way to read it back, only to revoke (`scripts/onyx-token.sh --revoke <id>`)
and mint another. The password is read interactively and never lands in argv
or on disk.

Confirm it took:

```bash
curl -fsS localhost:8080/api/v1/system/health | jq .onyx_api
# → {"configured": true, "status": "ok", "version": "v4.3.4", ...}
```

`status: "unauthorized"` means the token was rejected — health checks it
against Onyx up front rather than waiting for the first action to fail.

## Verify the install

```bash
curl -fsS localhost:8080/api/v1/system/health | jq .status   # "ok"
ovis status                                                  # same, prettier
open http://localhost:8080                                   # the dashboard
```

Then read the [Web UI guide](./web-ui.md) or the [CLI guide](./cli.md).
Something wrong? [Troubleshooting](./troubleshooting.md).
