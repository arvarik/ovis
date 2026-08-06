# Pruning

Removing documents that should not be in the corpus — duplicates, stubs,
foreign-language pages, junk sections — safely, reversibly, and without ever
disrupting Onyx.

Deletion is the most dangerous operation in the system: fanned out across
Postgres and the search index, and at pruning scale a mistake is thousands of
documents. So the center of gravity here is the **lifecycle**, not the
detection: nothing is ever deleted by a button or a command. Every document
passes through a reversible waiting room, and what the reaper deletes goes to
the trash, where it stays restorable.

## The lifecycle

```
scan → candidate → stage → staged (hidden, grace countdown)
                 ↘ dismiss           ↘ restore (exact)
                                     ↓
                        reaper: snapshot + cascade
                                     ↓
              TRASH (gone from Onyx, restorable) → purged after retention
                                     ↖ restore (full re-insert)
```

Two recovery windows, not one. A staged document is intact and merely hidden,
so restoring it is a flag flip. A trashed document is genuinely gone from Onyx
— its rows are deleted and its chunks removed from the index — but OVIS holds a
complete snapshot, so restoring it re-inserts everything including the
embedding vectors. The combined window is `OVIS_PRUNE_GRACE_DAYS +
OVIS_TRASH_RETENTION_DAYS` (7 + 30 by default).

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
| `quality` | text failing published quality gates | reads chunk text | Gopher/FineWeb/C4 thresholds. Flags only when ≥3 gates fail across ≥2 *families*, and never auto-stages — see the caveat below |
| `url_junk` | image/media/archive URLs indexed as pages | cheap | PDFs are **not** assets here: 88k of them are real content. Also emits `url_variant_of` |
| `url_variant` | documents sharing a canonical URL | cheap (uses stored profiles) | folds tracking parameters, scheme, `www.`, trailing slashes, index filenames. Catches copies whose content hashes differ |

### What the quality gates can and cannot tell you

The gates are the published Gopher (arXiv 2112.11446), FineWeb (arXiv
2406.17557) and C4 (arXiv 1910.10683) heuristics, reproduced at their
published thresholds because this corpus is a web crawl — the population those
numbers were tuned on.

They identify text that is *structurally unusual*, which overlaps with, but is
not the same as, text that is worthless. Measured on the reference deployment:
API reference pages, syntax diagrams and directory-style documentation trip
several gates at once because code blocks and tables genuinely have the text
shape of junk. On one documentation connector 122 of 955 documents were
flagged, and most were legitimate syntax-reference pages.

Three things follow, all of them enforced rather than advisory:

- **Quality never auto-stages.** No shipped preset gives it an auto band, and a
  test asserts that.
- **Confidence is capped below certainty** (0.85), and grows only with the
  number of failures.
- **Failures must span gate families.** Line-shape gates
  (`unterminated_lines`, `short_lines`, `newline_ratio`) fire together on one
  underlying property, so three of them count as one observation. Requiring
  two families took the flag rate on a random corpus sample from 27% to 14%
  and dropped exactly the documents worth keeping.

`quality.exempt_connectors` is the escape hatch for a source that is
legitimately code or tables end to end.

A re-scan updates existing open candidates instead of duplicating them, and
closes candidates whose reasons no longer apply
(`resolved_reason: no_longer_matches`). Every candidate records the config
hash it was produced under, so a threshold change is visible, not silent.

Scans are checkpointed background jobs: they survive a server restart and
resume from their cursor (verified with a `kill -9` mid-scan on the reference
deployment), and they can be cancelled between pages. One scan runs at a
time.

## Measurements, not verdicts

A scan writes an `ovis.doc_profile` row for **every** document it examines,
flagged or not: word counts, which quality gates failed, canonical URL, URL
class, strongest measured similarity. Verified duplicate pairs go to
`ovis.dup_pair` with their similarity, including pairs below the acting
threshold. Duplicate-group membership lives in `ovis.doc_dup_group`, one row
per `(document, method)` — a page can be both a content duplicate and a URL
variant, and each group keeps its own keeper.

That is what makes thresholds a review-time decision. `POST /prune/simulate`
evaluates a policy against the stored profiles and reports what it *would*
flag — as a real aggregate query, not an estimate — without creating anything.
Lowering a threshold and re-simulating costs a query; under v1 it cost a
re-scan of 1.7 M documents.

Simulation also reports what it cannot see. If no document has an embedding
similarity yet, a policy with semantic thresholds says so rather than
reporting a confident zero.

A policy's `cross_connector_review_only` (on in every shipped preset) keeps
duplicates whose group spans connectors out of the bulk band — they still
surface for review, they just stop being something automation stages. FineWeb's
finding is that global dedup over-prunes: a page mirrored across sources is
usually popular rather than redundant. The scan records the connector spread as
it builds the group, so the rule costs nothing at review time, and a group whose
connector cannot be determined counts as spanning.

A policy has three shipped presets (`conservative`, `standard`, `aggressive`),
but every signal is editable and any policy can be saved under a name and made
the deployment's active one. `POST /prune/policies/commit` turns a band into
candidates — review rows and nothing more. Staging is still a separate confirmed
action, and deletion still waits out the grace period; committing a policy is
the *cheapest* thing in the lifecycle, not a shortcut past it.

## Reviewing at scale

207,230 candidates is not a list anyone works through. The review surfaces
therefore make the *group* the unit of decision, not the row:

- **The funnel** (`/prune/overview`) reports the backlog as bundles by reason
  and by connector, each with its document count, the chunks it holds — the
  index weight deleting it actually reclaims — and how many sit on a
  still-crawling connector.
- **Threshold review.** `/prune/histogram` returns the measured distribution of
  any signal a policy can threshold on, so a number is chosen against what the
  corpus actually looks like rather than by feel.
- **Acceptance sampling** (`/prune/sample`) draws a random subset server-side —
  so a client cannot pick an easy one — and states the claim accepting it would
  support: with zero mistakes in `n` independent draws, the true error rate is
  below `1 - (1 - c)^(1/n)` at confidence `c`. The sentence travels with the
  numbers, because the decision it feeds is a human's.
- **Cluster review** (`/prune/clusters`) returns whole duplicate groups, keeper
  first, with the rule that chose the keeper. 49,683 hash groups is a reviewable
  number of decisions; the 184,058 candidates inside them are not.

All of this is read-only. Nothing on this path hides or deletes anything.

## The reaper

The reaper ticks every `OVIS_PRUNE_REAPER_INTERVAL_SECS` (300) and is the
**only** code path that hard-deletes pruned documents:

- **Snapshots before it deletes.** The snapshot and the Postgres cascade share
  one transaction, so the only two outcomes are "document present, no
  snapshot" and "document gone, snapshot exists". A document whose chunks
  cannot be read is not deleted at all.
- **Refuses to run without a trash.** If `ovis.trash_document` could not be
  created the reaper halts with `trash_unavailable` rather than falling back to
  irreversible deletion.

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

## The trash

Everything the reaper deletes lands in `ovis.trash_document` first:

| Captured | Why |
|---|---|
| the full `public.document` row | restore has to rebuild it exactly |
| tags and connector attribution | otherwise a restored document loses its provenance |
| every chunk's verbatim OpenSearch `_source` | the content itself |
| embedding vectors, packed as f16 | a restored document is semantically searchable **immediately**, with no re-index |

About 15 kB per document with vectors, 5 kB without
(`OVIS_TRASH_KEEP_VECTORS=false`). Onyx cannot see any of it: the document rows
are genuinely deleted and the chunks genuinely removed, so search, connectors
and the Onyx admin UI have nothing left to find. The bytes live in the `ovis`
schema, which Onyx never reads.

- **Restore** re-inserts the Postgres rows and bulk-indexes the chunks under
  their original ids. The `hidden` flag comes back as the document's own
  `prev_hidden` — the value it carried *before* pruning touched it — because
  staging's flag is part of the pruning process, not part of the document.
  Tags whose `tag` row has since been deleted, and attributions whose
  connector is gone, are skipped and **counted** in the response rather than
  silently dropped.
- **Reappeared documents**: if the crawler brought the id back, restore is
  refused unless `overwrite` is set. The Trash tab badges these.
- **Hold** pins a snapshot indefinitely, exempt from automatic purge.
- **Purge** is the only genuinely irreversible operation in the system. It
  requires the typed count at *every* size, skips held snapshots, and there is
  deliberately no "empty trash" verb.

Verified end to end against the live deployment: a document was staged,
deleted by the reaper, confirmed absent from Postgres and from all 4 of its
index chunks, then restored — with its vectors intact (unit norm preserved
through the f16 round trip) and returned as the top kNN hit for its own
embedding.

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
| `OVIS_TRASH_RETENTION_DAYS` | 30 | how long a deleted document stays restorable (1–365; zero is refused — a trash that empties at once is not a trash) |
| `OVIS_TRASH_KEEP_VECTORS` | true | capture embeddings, so restore needs no re-index |
| `OVIS_TRASH_PURGE_BATCH_SIZE` | 200 | expired snapshots purged per reaper cycle |

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

## Performance

Measured against the reference deployment (1.74 M documents, LAN):

| Phase | Rate | Full corpus |
|---|---|---|
| SQL-only detectors (`thin`, `url_junk`) | ~1,700 docs/s | ~17 min |
| With text (`quality`, `near_duplicate`, `language`) | ~380 docs/s | ~76 min |

The text pass batches a whole page of documents into one OpenSearch
`_msearch` rather than a query per document; like-for-like on the same
connector that is a 2.8× speedup, and it is what brings a full-corpus text
scan under the hour-and-a-quarter mark. Candidate writes are batched per page
for the same reason.

Long documents are slower per row (a 900-document philosophy encyclopedia
measured ~96 docs/s), because that phase is bound by text volume rather than
by round trips.

## Upgrading

A scan records the configuration it was queued under, and reads it back
strictly — a scan must run under exactly the settings it was created with. An
**older** OVIS instance sharing the database with a newer one therefore cannot
run a scan queued by the newer one; it logs `scan_deferred_version`, leaves the
scan untouched, and lets the newer instance pick it up. It does not consume
the retry budget and does not mark the scan failed. Observed during a live
rolling upgrade.

## What pruning never touches

Test-enforced, not aspirational: `index_attempt` park sentinels,
`search_settings`, connector configuration, any Onyx table outside the
existing delete cascade. Pruning never triggers crawls and never fetches
source URLs — detection reads only what is already indexed.
