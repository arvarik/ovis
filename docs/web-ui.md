# The web UI

The dashboard served at `/` — a React app embedded in the backend binary.
Mobile-first with one component tree, keyboard-driven on desktop, and honest
everywhere: what the API states as uncertain or risky renders as exactly that.

![The Explorer](./screenshots/explorer.jpg)

## The six views

### Pages — the Explorer (`/pages`)

Browse, filter, and search the whole corpus. Server-side preset chips (All /
Stubs / Heavy / Recent / Hidden) whose counts are **global truths** scoped to
your active filters — never counted over the visible page. Infinite scrolling
rides keyset cursors, so there is no depth limit. The list is a table on wide
containers and cards on narrow ones — decided by the *container*, not the
viewport, so it adapts when the inspector splits the screen.

Typing in the search pill filters titles/URLs live (debounced into the URL —
every view state here is shareable and survives refresh/back). **Enter**
submits a content search instead:

![Content search with the degradation chip](./screenshots/search.jpg)

Content search shows score bars, highlighted snippets, a mode switch
(keyword / semantic / hybrid), and — when the index can't serve a vector mode
— the degradation chip stating what actually ran. An optional **Live** mode
streams the filtered list over SSE and names the server's cap when it
truncates rather than posing as complete.

### The Inspector (`/pages/…`)

A side panel on desktop, a bottom sheet on phones; deep-linkable (document ids
are URLs and round-trip percent-encoded through the route). Overview with the
full honest metadata, reconstructed **Text** (markdown-aware, find-in-text,
download), **Chunks** with a "Load vector" that fetches one *real* stored
vector (dimension and model from the response — never fabricated), and the raw
**JSON** as a collapsible tree.

![The Inspector](./screenshots/inspector.jpg)

Edit covers title, boost (−4…+8) and hidden; the toast reports whether the
change was applied via the Onyx API and whether the index synced. Delete
spells out the consequences — chunk count, the no-undo truth, and a recrawl
warning when the owning connector is active — and offers **hide instead**,
the reversible alternative.

### Connectors (`/connectors`)

The fleet health matrix: true cc-pair statuses, real document counts, parked
badges, repeated-error indicators; filter by the summary tiles, source, or
name; multi-select for pause/resume.

![The connector fleet](./screenshots/connectors.jpg)

The detail view adds the config card (proxy-routed URLs get a "via proxy"
chip), a 7-day docs-added sparkline, attempt history with expandable error
messages, the rolling-window error list with a failed-doc reindex action, and
the pair's authoritative document list.

![Connector detail](./screenshots/connector-detail.jpg)

Two guardrails surface here: **Run now** on a parked connector opens an
explainer requiring an explicit "I understand" (the park sentinel means the
crawl was finished *on purpose*), and **Delete** requires typing the
connector's exact name.

### Activity (`/activity`)

What the crawlers are doing right now — replaces ssh + psql. Live cards per
running attempt: pages/min, batch progress, heartbeat freshness; auto-refresh
every 5 s while visible, with a pause toggle. Queued attempts are labeled
**queued** (normal), and **stalled** comes only from the backend's
no-heartbeat-for-45-minutes heuristic — never from document counts.

![Activity](./screenshots/activity.jpg)

### Stats (`/stats`)

The at-a-glance corpus dashboard: document/chunk totals (estimates marked
`~`), the crawl timeline, sources, attempt outcomes, top connectors, a disk
gauge with the OpenSearch read-only alarm state, and a runtime card whose
every value is live from the API — nothing hardcoded.

![Stats](./screenshots/stats.jpg)

### Prune (`/prune`)

Review-first junk removal — the full guide is [pruning.md](./pruning.md).
Four tabs under an always-visible status strip (candidates open · staged with
the soonest grace countdown · deleted this week · reaper state, which turns
rose when halted and gold when deferring):

- **Review** — the scan launcher (scope picker, detector checklist, one "Dry
  scan" button that cannot mutate; progress renders live and survives
  restarts), then the candidate list: reason chips (`dup 94%`, `lang deu
  0.98`, `rule: calendar-pages`, `stub`), the confidence as a number, recrawl
  badges, filters and bulk selection. The detail drawer puts the evidence
  first — duplicate candidates show both documents side by side with the
  keeper labeled.
- **Staged** — the waiting room: live countdowns, Restore (instant, exact, no
  confirmation) and "Delete sooner…" (still reaper-executed). Staged
  documents are hidden from Onyx search but fully intact; they delete
  automatically when their grace ends.
- **Rules** — URL/tag pattern CRUD with a live-data **Preview**; the enable
  switch waits until a rule has been previewed. Detector config exports and
  imports as YAML.
- **History** — the audit trail, filterable, with per-batch outcomes
  (`chunks_deleted`, any `index cleanup pending`) rendered honestly.

Staging over the server's big-batch limit requires typing the count into the
confirmation, and every bulk action sends `confirm_count` — if the set
changed on the server, nothing happens and the fresh count is shown.

## Keyboard

`?` shows every binding for the current screen, rendered from the same
registry that dispatches them — help cannot drift from behaviour.

| Key | Action |
|---|---|
| `⌘K` | command palette (actions, connector jump, recent documents, content search) |
| `/` | focus search |
| `j` `k` / arrows | move the active row |
| `Enter` / `o` | inspect / open the link |
| `x` / `Shift+X` | select / range-select |
| `[` `]` | previous / next document in the inspector |
| `d` | delete (from the inspector, with the confirm dialog) |
| `g` then `p/c/a/s` | go to Pages / Connectors / Activity / Stats |
| `r` | refresh data |
| `Esc` | closes exactly one layer — the topmost |

![The command palette](./screenshots/palette.jpg)

## On a phone

Every view works one-handed at 375 px: bottom tabs, a full-screen search sheet,
44 px touch targets, safe-area aware, no horizontal scrolling. Add it to your
home screen — the manifest makes it a standalone dark app.

<img src="./screenshots/mobile.jpg" alt="Mobile" width="320">

## What the interface promises

- **A failed request is an error state with a retry** — never an empty list
  posing as success. Errors carry the API's own code and request id.
- **Nothing is fabricated**: no fake vectors, no fake undo, no placeholder
  numbers. `chunk_count: null` renders as "not counted yet", estimates carry
  `~`, orphaned chunks get a banner, recrawl risk is stated before you delete.
- **Mutations are optimistic with rollback** — rows return visibly if the
  server refuses, and partial batch failures restore everything except what
  the server confirmed deleted.

Developer workflow (building, checks, the screenshot/axe drivers):
[`ui/README.md`](../ui/README.md).
