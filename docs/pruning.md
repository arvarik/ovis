# Pruning

Removing documents that should not be in the corpus — duplicates, stubs,
foreign-language pages, junk sections — safely, reversibly, and without ever
disrupting Onyx.

Deletion is the most dangerous operation in the system: irreversible, fanned
out across Postgres and the search index, and at pruning scale a mistake is
thousands of documents. So the center of gravity here is the **lifecycle**,
not the detection: nothing is ever deleted by a button or a command. Every
document passes through a reversible waiting room first, and only a
background task (the reaper) ever runs the cascade.

## The lifecycle

```
scan → candidate → stage → staged (hidden, grace countdown) → reaper deletes
                 ↘ dismiss                ↘ restore (exact)
```

- A **scan** is a preview. It examines documents and produces *candidates*
  with per-document reasons, confidence and evidence. It never mutates a
  document; you can run one against the whole corpus with no risk.
- **Staging** is the only mutation review can perform, and it is the
  reversible `hidden` flag — set through the Onyx API when a token is
  configured, so Onyx keeps its own index in sync. A staged document serves
  no search results but keeps every byte.
- Staging starts the **grace period** (`OVIS_PRUNE_GRACE_DAYS`, default 7).
  When it ends, the reaper deletes the document through the same FK-complete
  cascade as `DELETE /pages/{id}`. Until that moment, **restore** is one
  click/command and returns the document exactly as it was — including
  returning a document that was already hidden before staging to
  hidden-but-unstaged.
- **Dismiss** closes a candidate as not-junk. With "never flag again"
  (`--forever`), the document also lands on the exclusion list and no scan
  will ever re-flag it.
- **Schedule-delete** (`ovis prune delete`, "Delete sooner…" in the UI) never
  deletes inline. On open candidates it stages them — the full grace period
  applies. On already-staged documents it brings the deadline forward to
  now; the reaper still executes, and restore still works until the cascade
  actually runs. There is no `--now`.

## Detectors

Scans run only the detectors you name. All thresholds live in the detector
config (`ovis prune config export`), and every default is conservative.

| Detector | Finds | Cost | Notes |
|---|---|---|---|
| `exact_duplicate` | identical `content_hash` groups | pure Postgres | keeper by `dedup.prefer_keep` (default `shortest_url`); every other member is a candidate at confidence 1.0 |
| `near_duplicate` | MinHash/LSH near-copies | reads chunk text | threshold 0.90; the 0.80–0.90 band is surfaced as report-only low confidence. Signatures are persisted, so re-scans only recompute changed documents |
| `thin` | 0-chunk stubs; near-empty pages | Postgres; words check reads text | `chunk_count = 0` age-gated by `thin.min_age_days` (7 d). `chunk_count: null` is "not counted yet" and is **never** flagged |
| `language` | pages outside `language.allowed` | reads chunk text | ships **disabled** — multilingual corpora are legitimate. Per-connector opt-outs supported |
| `url_rule` / `tag_rule` | URL or tag patterns you author | cheap | rules start disabled; preview them against live data first |
| `stale` | old pages on still-active connectors | cheap | policy, not junk; ships disabled |

A re-scan updates existing open candidates instead of duplicating them, and
closes candidates whose reasons no longer apply
(`resolved_reason: no_longer_matches`). Every candidate records the config
hash it was produced under, so a threshold change is visible, not silent.

Scans are checkpointed background jobs: they survive a server restart and
resume from their cursor (verified with a `kill -9` mid-scan on the reference
deployment), and they can be cancelled between pages. One scan runs at a
time.

## The reaper

The reaper ticks every `OVIS_PRUNE_REAPER_INTERVAL_SECS` (300) and is the
**only** code path that hard-deletes pruned documents:

- Deletes only staged documents whose grace has ended, oldest deadline
  first, in batches of `OVIS_PRUNE_REAPER_BATCH_SIZE` (100) with a
  `OVIS_PRUNE_REAPER_PAUSE_MS` (2000) breather between batches, capped at
  `OVIS_PRUNE_MAX_DOCS_PER_HOUR` (2000) per trailing hour. The index has
  tripped disk watermarks before; deletion pressure is deliberately gentle.
- **Defers** documents whose cc-pair has an `IN_PROGRESS` index attempt
  (`deferred: indexing_in_progress` on `/prune/status`) — deleting under an
  active writer invites re-insert races. They stay staged; the next cycle
  retries.
- **Halts** outright when the index reports a read-only block (or its status
  cannot be verified), and says so: `halted: index_read_only` in the status,
  rose banner in the UI, exit 13 from `ovis prune status`. Deleting into a
  read-only index only queues cleanup debt.
- Uses the same cascade as the interactive delete paths, with the same
  honesty: per-document `chunks_deleted`, and failed index cleanup queued in
  `ovis.pending_index_deletes` (`index_cleanup_pending: true`) rather than
  silently orphaned.
- Survives crashes without double-deleting: a document is claimed by a
  state flip (`staged → deleting`), and on restart leftover `deleting` rows
  are re-verified — an intact document goes back to staged; a half-deleted
  one is closed honestly with index cleanup queued.

## Recrawl risk, handled honestly

Deleting from an ACTIVE connector is likely undone at its next scheduled
refresh (~30 days for web connectors here). Pruning does not pretend
otherwise:

- Every candidate carries `recrawl_risk`, derived from the owning cc-pair
  status. Both surfaces badge it and the delete confirmations break it out.
- Deleting with **remember** (default for risky documents in the API) records
  the document on the exclusion list. When the crawler brings it back, the
  reaper automatically **stages** the new copy — hidden, full grace period,
  normal lifecycle, audited as `restaged_recrawled`. Automation never skips
  the waiting room.
- The honest recommendation: prune freely from PAUSED and parked connectors
  (durable today); for ACTIVE pairs expect the reaper to chaperone repeats,
  or pause the pair first.

## Guardrails

- Every bulk mutation resolves its selection server-side and requires a
  matching `confirm_count`; if the set changed underneath you the request is
  a 409 carrying the fresh count and **nothing** happens.
- Batches beyond `OVIS_PRUNE_BIG_BATCH` (500) require typing the count on
  both surfaces. `-y` does not waive it, and neither does `--filter` delete.
- `--no-input` turns any prompt into exit 10, as everywhere in the CLI.
- Restore and dismiss are the safe directions and need no confirmation.
- With `OVIS_API_TOKEN` set every prune endpoint requires the bearer token —
  pruning is the strongest argument yet for setting it.

## Audit

`ovis.prune_audit` records every transition — staged (by whom), restored,
dismissed, scheduled, deleted (with per-document outcome), deferred, halted,
scan lifecycle — and is never deleted by OVIS. The UI History tab and
`ovis prune log` read the same rows.

## Configuration

Server settings (environment, all optional):

| Variable | Default | Meaning |
|---|---|---|
| `OVIS_PRUNE_GRACE_DAYS` | 7 | staged → deletable delay (0–90; 0 still goes through the reaper) |
| `OVIS_PRUNE_REAPER_INTERVAL_SECS` | 300 | reaper cycle |
| `OVIS_PRUNE_REAPER_BATCH_SIZE` | 100 | documents per delete batch (≤ `OVIS_BATCH_DELETE_MAX`) |
| `OVIS_PRUNE_REAPER_PAUSE_MS` | 2000 | pause between batches |
| `OVIS_PRUNE_MAX_DOCS_PER_HOUR` | 2000 | hard hourly deletion ceiling |
| `OVIS_PRUNE_BIG_BATCH` | 500 | typed-count threshold on both surfaces |
| `OVIS_PRUNE_SCAN_PAGE_SIZE` | 1000 | scan keyset page |

Detector configuration is data, not env: it lives in `ovis.prune_rules` and
round-trips as YAML (`ovis prune config export|import`, or the Rules tab).
URL/tag rules are individual rows with a live **preview** before enabling.

For the full-corpus `exact_duplicate` scan to page efficiently, apply the
`ix_ovis_document_content_hash` support index from
[`ops/onyx_indexes.sql`](../ops/onyx_indexes.sql) (79 MB on the reference
corpus).

## A worked session (CLI)

```bash
ovis prune scan -c microbiology-info -d exact_duplicate -d thin   # preview
ovis prune ls                                                     # review with reasons
ovis prune show @3                                                # evidence, pairs side by side
ovis prune dismiss @5 --forever                                   # not junk, never re-flag
ovis prune stage @1 @2 @3                                         # hide, grace starts
ovis prune staged                                                 # the waiting room
ovis prune restore @2                                             # changed your mind — exact restore
ovis prune delete @1 --remember                                   # bring the deadline forward
ovis prune status                                                 # counts, reaper state, rates
ovis prune log --since 1d                                         # who did what
```

## What pruning never touches

Test-enforced, not aspirational: `index_attempt` park sentinels,
`search_settings`, connector configuration, any Onyx table outside the
existing delete cascade. Pruning never triggers crawls and never fetches
source URLs — detection reads only what is already indexed.
