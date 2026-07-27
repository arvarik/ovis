# OVIS documentation

Everything needed to run, use, operate, and hack on OVIS.

| Guide | What it covers |
|---|---|
| [Getting started](./getting-started.md) | Prerequisites, install (Docker or from source), configuration, the index migration, the Onyx token, first run |
| [Architecture](./architecture.md) | The design: one data plane, who talks to what, the crates, the honesty principles, the reference deployment |
| [HTTP API](./api.md) | Every endpoint, pagination, SSE, the error envelope, the honest fields, auth |
| [CLI](./cli.md) | `ovis` — commands, `@N` handles, output formats, exit codes, config, the TUI |
| [Web UI](./web-ui.md) | The five views, keyboard shortcuts, mobile, what the interface promises |
| [Operations](./operations.md) | Production settings, health & metrics, performance, security, day-2 concerns |
| [Troubleshooting](./troubleshooting.md) | Symptom → cause → fix, for everything we have actually seen go wrong |
| [Development](./development.md) | Building, testing (unit / DB-backed / live smoke / bench), UI workflow, quality gates |

Two more places worth knowing about:

- [`redesign/`](../redesign/) — the full design archive: audits of the code this
  replaced, the specifications, and per-track `05_AS_BUILT.md` documents
  recording exactly where the shipped code deviates from the designs and why.
  The documentation you are reading describes *what is*; `redesign/` preserves
  *why it is that way*.
- [`.env.example`](../.env.example) — the authoritative, annotated list of
  every configuration setting.
