# `ovis-cli`

`ovis` — the command line and terminal UI.

**It is an API client.** It speaks the OVIS HTTP API and holds no database or
OpenSearch credentials; `ovis-core`'s `db` and `search` modules are the
*backend's* data plane and are unreachable from here. The user guide is
[`docs/cli.md`](../../docs/cli.md).

## The hot path

```bash
ovis status                          # server + dependency health at a glance
ovis p ls kant --sort chunks:desc    # page → p, list → ls
ovis p view @2                       # @N refers to a row of your last list
ovis p text @2                       # full text through $PAGER
ovis p open @2                       # in a browser
ovis p rm @3                         # prompts with title, URL, chunks, recrawl risk
```

`@N` handles are the navigation currency: every list and search prints them,
every id-taking command accepts them. They live in
`$XDG_STATE_HOME/ovis/last-list.json` and expire after an hour, so `@3` can never
quietly mean a different document than the one you looked at.

The footer always teaches the next step — the exact next-page command, the verbs
that apply — so nothing has to be memorised.

## Nouns

```
page        list view text chunks open edit delete search
connector   list view docs attempts errors pause resume run prune delete
search      content search over the chunk index
stats       overview | connectors | timeline | sources
status      server and dependency health          (exit 0 healthy, 13 degraded)
tui         full-screen browser
server      start stop restart status setup-onyx-key
config      init show set path
completions bash | zsh | fish
```

`ovis <noun> --help` for the flags. Aliases: `p`/`pages`, `c`/`connectors`,
`ls`/`list`, `show`/`inspect`/`view`, `rm`/`delete`.

## Rules it keeps

- **Global flags are global.** `--server --token -o/--format --color -q -v
  --no-input -y --profile --wide --columns --no-headers` work anywhere on the
  line, before or after any subcommand.
- **stdout is data, stderr is diagnostics.** `ovis page list -o json | jq .` is
  always clean; info lines, warnings and footers never touch stdout.
- **`--format json` is the wire struct**, byte-for-byte what the API returned.
- **Nothing is fabricated.** There is no sample data to fall back to. A failure
  is an error with a non-zero exit code.
- **Mutations report what happened** — `pg_deleted`, `chunks_deleted`,
  `index_cleanup_pending`, `recrawl_risk`, per-item `failed[]` — rather than
  printing "success" over the top.

### Exit codes

| | | | |
|---|---|---|---|
| `0` success | `1` generic | `2` usage | `3` not found |
| `10` confirmation needed under `--no-input` | `11` partial failure | `12` server unreachable | `13` degraded |
| `14` stale `@N` handle | | | |

## Guardrails

The backend enforces these; the CLI surfaces them rather than routing around
them.

- **Parked connectors.** `connector run` on a cc-pair the resilience cron parked
  explains what parked means and asks. `--acknowledge-parked` is never set on
  your behalf.
- **Connector delete** requires the name typed back, and `-y` does *not* skip it
  — the operation can destroy a hundred thousand documents.
- **There is no bulk crawl trigger.** `run` takes exactly one connector, by
  grammar; `pause`/`resume` take many.
- **`--no-input`** turns any prompt into exit 10 instead of an assumed yes.

## Configuration

```bash
ovis config init                 # annotated ~/.config/ovis/config.toml
ovis config show --origin        # every effective value and where it came from
ovis config set profiles.homelab.server http://192.168.4.113:8080
```

Precedence is **flags > environment > profile > defaults**
(`OVIS_SERVER`, `OVIS_TOKEN`, `OVIS_PROFILE`, `OVIS_CONFIG`, `NO_COLOR`).
`--origin` exists because a config system you cannot interrogate is one you end
up debugging with `strace`.

The `[server]` section is separate from `[profiles]` on purpose: the client needs
a URL, the server needs credentials, and the two should never be confused.

## Hosting the backend

The same binary hosts the API, so a single-file homelab deploy works:

```bash
ovis config set server.database_url   'postgres://…@192.168.4.113:5433/postgres'
ovis config set server.opensearch_url 'http://192.168.4.113:9200'
ovis server start -d                 # logs to $XDG_STATE_HOME/ovis/server.log
ovis server status                   # 0 healthy · 13 degraded · 12 not running
ovis server stop                     # SIGTERM, wait 10s, SIGKILL, verify
```

`start -d` waits for the server to answer before reporting success, and quotes
the reason from the log if it dies. `status` decodes the health body, so a
foreign process on the port is reported as foreign rather than as a healthy OVIS.

## The TUI

```
[1] Pages       browse, filter, content-search, inspect, mark, delete
[2] Connectors  status, config, attempts sparkline, pause/resume/run, errors
[3] Activity    live index attempts, crawl rate, OpenSearch disk gauge
```

`?` lists every binding for the current screen — from the same table that
dispatches them, so help cannot drift from behaviour. Data fetches run on a
worker task and never block a frame; deletes go through the API and a row
disappears only after the server confirms it.

## Layout

```
src/
├── api.rs        the ApiClient — the only data path
├── cli.rs        the clap tree (every global flag `global = true`)
├── config.rs     file, profiles, and origin-tracked resolution
├── error.rs      CliError -> message + hint + exit code
├── handles.rs    @N handles
├── output/       formats, colour, paging, tables, CSV
├── render.rs     wire structs -> grids, one place per entity
├── resolve.rs    ID|NAME -> connector, --since, --chunks, --sort
├── sse.rs        the `--all` stream reader
├── picker.rs     fuzzy matching, shared with the TUI
├── prompt.rs     confirmations, read from /dev/tty
├── commands/     one module per noun
└── tui/          keys · data worker · screens · widgets
```

## Tests

```bash
cargo test -p ovis-cli           # 187 unit + 30 binary-level, no services needed
```

The binary-level suite (`tests/cli.rs`) runs the real executable against
`wiremock`: exit codes, stdout purity, error envelopes, the `@N` lifecycle,
delete confirmation flows, and the paths that must not be exercised against live
data. Every run gets an isolated config and state directory.
