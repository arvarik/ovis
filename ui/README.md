# `ui/`

The OVIS web dashboard — React 19 + TypeScript + Tailwind v4, built by Vite and
embedded into the backend binary by `rust-embed`. Design in
[`redesign/frontend/`](../redesign/frontend/), and
[`05_AS_BUILT.md`](../redesign/frontend/05_AS_BUILT.md) for where the shipped
code deviates from it.

**It is an API client.** Everything it shows comes from `/api/v1`; nothing is
hardcoded, fabricated, or smoothed over. The API's honest fields —
`chunk_count: null`, `total_exact: false`, `degraded`, `pg_row`,
`recrawl_risk`, `parked`, `stalled` — render as what they mean.

## Developing

```bash
npm install
npm run dev        # Vite on :3000 (falls back to :3001), /api proxied to :8080
```

Run the backend on `:8080` first; every view is built against live data.

## Shipping

```bash
npm run build      # tsc -b + vite build -> dist/
cargo build --release -p ovis-backend   # embeds dist/ into the binary
```

`ui/dist` is a build artifact and fully untracked; ovis-backend's `build.rs`
creates the folder when absent, so a fresh clone compiles (serving "UI assets
are not embedded") before the first `npm run build`.

## Checks

```bash
npm run typecheck        # tsc -b
npm run lint             # eslint
npm run lint:tokens      # off-token color usage fails the build
npm test                 # vitest (hotkey layering, URL state, snippet safety, presets)
node scripts/axe-audit.mjs    # WCAG 2 A/AA/2.1AA across the five routes, non-zero on violations
node scripts/drive.mjs <url> <prefix> [w h] ['click=…;press=…;shot=…']   # headless screenshots
```

The two driver scripts use `playwright-core` against the system Chrome — no
browser downloads.

## Layout

```
src/
├── api/          typed client (error envelope, percent-encoded doc ids),
│                 wire-type mirror of ovis-core::api_types, queries, mutations, SSE
├── components/
│   ├── primitives/   Button, Badge, Sheet, Dialog, Menu, Tabs, … (tokens only)
│   ├── shell/        TopBar, NavRail, BottomTabs, SearchPill, palette, health dot
│   ├── documents/    Explorer, DocumentList, filters, presets, Inspector
│   ├── connectors/   fleet, detail, activity, action dialogs (parked guard, delete-by-name)
│   └── stats/        dashboard, charts (recharts, lazy-loaded)
├── hooks/        layered hotkeys (+ the ? overlay's data), media query, container width
├── lib/          cn, formatting, recent docs/searches
├── routes/       TanStack Router routes; every filter/sort/query lives in the URL
└── styles/       theme.css — the single source of color; the default Tailwind
                  palette is deleted, so off-token classes produce no CSS
```

Design rules that are enforced, not aspirational: mobile-first with one
component tree (no `Mobile*`/`Desktop*` forks), 44 px touch targets below `md`,
document ids always percent-encoded as one path segment, mutations always
optimistic-with-rollback and honest in their toasts, and `/lab` renders every
primitive in both viewport classes.
