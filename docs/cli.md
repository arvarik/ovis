# The CLI

`ovis` — the command line and terminal UI. It is an **API client**: it speaks
the OVIS HTTP API and holds no database or OpenSearch credentials. Point it
somewhere with `--server`, `OVIS_SERVER`, or a config profile.

```bash
cargo build --release -p ovis-cli        # or: cargo install --path crates/ovis-cli
ovis status
```

## The hot path

```bash
ovis status                          # server + dependency health at a glance
ovis p ls kant --sort chunks:desc    # aliases: page → p, list → ls
ovis p view @2                       # @N refers to a row of your last list
ovis p text @2                       # full text through $PAGER
ovis p open @2                       # in a browser
ovis p rm @3                         # prompts with title, URL, chunks, recrawl risk
ovis search kant --mode hybrid       # says so when it degrades to keyword
ovis c ls --parked                   # connectors the resilience cron parked
ovis connector run istio-blog        # one cc-pair; there is no bulk trigger
ovis tui                             # full-screen: pages / connectors / activity
```

**`@N` handles are the navigation currency.** Every list and search prints
them; every id-taking command accepts them. They live in
`$XDG_STATE_HOME/ovis/last-list.json` and expire after an hour, so `@3` can
never quietly mean a different document than the one you looked at (a stale
handle is exit 14, not a wrong document). The footer always teaches the next
step — the exact next-page command, the verbs that apply.

## Command tree

```
page        list view text chunks open edit delete search
connector   list view docs attempts errors pause resume run prune delete
search      content search over the chunk index
stats       overview | connectors | timeline | sources
status      server and dependency health          (exit 0 healthy, 13 degraded)
tui         full-screen browser
server      start stop restart status setup-onyx-key
config      init show set path
completions bash | zsh | fish       (completes connector names live)
```

Aliases: `p`/`pages`, `c`/`connectors`, `ls`/`list`, `show`/`inspect`/`view`,
`rm`/`delete`. `ovis <noun> --help` for every flag.

## Rules it keeps

- **Global flags are global.** `--server --token -o/--format --color -q -v
  --no-input -y --profile --wide --columns --no-headers` work anywhere on the
  line, before or after any subcommand.
- **stdout is data, stderr is diagnostics.** `ovis page list -o json | jq .`
  is always clean; warnings, footers and progress never touch stdout.
- **`--format json` is the wire struct**, byte-for-byte what the API returned
  — including `GET /connectors`' bare array.
- **Nothing is fabricated.** There is no sample data to fall back to; a
  failure is an error with a non-zero exit code, never plausible output.
- **Mutations report what happened** — `pg_deleted`, `chunks_deleted`,
  `index_cleanup_pending`, `recrawl_risk`, per-item `failed[]`.
- **`--all` is honest about server caps.** It streams via SSE, pre-flights the
  true total, and if the server's stream ceiling truncated the result it says
  exactly how many of how many arrived — and exits 11.

### Exit codes

| | | | |
|---|---|---|---|
| `0` success | `1` generic | `2` usage | `3` not found |
| `10` confirmation needed under `--no-input` | `11` partial failure | `12` server unreachable | `13` degraded |
| `14` stale `@N` handle | | | |

## Output formats

`table` (default, colour-aware, honest column truncation — the URL column is
never truncated), `json`, `ndjson`, `csv`, `yaml`. All except YAML stream with
a flat memory profile; `--all` refuses YAML (one document would mean buffering
everything) and points at `ndjson`.

## Guardrails

The backend enforces these; the CLI surfaces them rather than routing around
them:

- **Parked connectors.** `connector run` on a parked cc-pair explains what
  parked means and asks. `--acknowledge-parked` is never set on your behalf.
- **Connector delete** requires the name typed back, and `-y` does *not* skip
  it — the operation can destroy a hundred thousand documents.
- **No bulk crawl trigger.** `run` takes exactly one connector, by grammar;
  `pause`/`resume` take many.
- **`--no-input`** turns any prompt into exit 10 instead of an assumed yes.

## Configuration

```bash
ovis config init                 # annotated ~/.config/ovis/config.toml
ovis config show --origin        # every effective value and where it came from
ovis config set profiles.homelab.server http://192.168.4.113:8080
```

Precedence: **flags > environment > profile > defaults**
(`OVIS_SERVER`, `OVIS_TOKEN`, `OVIS_PROFILE`, `OVIS_CONFIG`, `NO_COLOR`).
`--origin` exists because a config system you cannot interrogate is one you
end up debugging with `strace`.

## Hosting the backend

The same binary hosts the API, so a single-file deploy works:

```bash
ovis config set server.database_url   'postgres://…@host:5433/postgres'
ovis config set server.opensearch_url 'http://host:9200'
ovis server start -d                 # waits for health; logs to $XDG_STATE_HOME/ovis/server.log
ovis server status                   # 0 healthy · 13 degraded · 12 not running
ovis server stop                     # SIGTERM, wait 10s, SIGKILL, verify
```

`start -d` reports success only after the server answers, and quotes the
reason from the log if it dies instead. `status` decodes the health body, so a
foreign process on the port reads as *foreign*, not as a healthy OVIS. The
`[server]` config section is separate from client `[profiles]` on purpose:
the client needs a URL, the server needs credentials, and the two should never
be confused.

## The TUI

```
[1] Pages       browse, filter, content-search, inspect, mark, delete
[2] Connectors  status, config, attempts sparkline, pause/resume/run, errors
[3] Activity    live index attempts, crawl rate, OpenSearch disk gauge
```

`?` lists every binding for the current screen — rendered from the same table
that dispatches them, so help cannot drift from behaviour. Data fetches run on
a worker task and never block a frame; deletes go through the API and a row
disappears only after the server confirms it.

More detail (internals, layout, tests):
[`crates/ovis-cli/README.md`](../crates/ovis-cli/README.md).
