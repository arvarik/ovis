//! The command tree.
//!
//! Every global flag is `global = true`. The old definition declared them on the
//! root only, so `ovis page list --format json` — an example from its own
//! documentation — was a parse error.

use clap::{Args, Parser, Subcommand};

use crate::output::Format;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "ovis",
    version,
    about = "Onyx Visibility — see and control what Onyx has crawled",
    long_about = "Onyx Visibility.\n\nThe CLI speaks the OVIS HTTP API and holds no database \
                  credentials; point it at a server with --server or OVIS_SERVER.",
    propagate_version = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(flatten)]
    pub globals: GlobalArgs,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Args, Debug, Clone, Default)]
pub struct GlobalArgs {
    /// OVIS backend base URL [env: OVIS_SERVER] [default: http://127.0.0.1:8080]
    #[arg(long, global = true, value_name = "URL")]
    pub server: Option<String>,

    /// Bearer token, if the server has auth on [env: OVIS_TOKEN]
    #[arg(long, global = true, value_name = "TOKEN")]
    pub token: Option<String>,

    /// Output format
    #[arg(short = 'o', long, global = true, value_name = "FORMAT")]
    pub format: Option<Format>,

    /// When to colourise output [default: auto] (NO_COLOR is always honoured)
    #[arg(long, global = true, value_name = "WHEN")]
    pub color: Option<crate::output::ColorChoice>,

    /// Suppress informational lines on stderr
    #[arg(short = 'q', long, global = true)]
    pub quiet: bool,

    /// Debug logging on stderr; repeat for more
    #[arg(short = 'v', long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Never prompt; an unconfirmed destructive operation fails with exit 10
    #[arg(long, global = true)]
    pub no_input: bool,

    /// Assume yes for confirmations
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,

    /// Config profile to use [env: OVIS_PROFILE]
    #[arg(long, global = true, value_name = "NAME")]
    pub profile: Option<String>,

    /// Show every column, and absolute timestamps
    #[arg(long, global = true)]
    pub wide: bool,

    /// Pick and order columns, e.g. --columns title,chunks,url
    #[arg(long, global = true, value_name = "LIST")]
    pub columns: Option<String>,

    /// Omit the header row
    #[arg(long, global = true)]
    pub no_headers: bool,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Browse, inspect and manage crawled pages
    #[command(alias = "pages", alias = "p")]
    Page {
        #[command(subcommand)]
        action: PageCommand,
    },

    /// Inspect and control Onyx connectors
    #[command(alias = "connectors", alias = "c")]
    Connector {
        #[command(subcommand)]
        action: ConnectorCommand,
    },

    /// Content search across the chunk index
    Search(SearchArgs),

    /// Deployment statistics
    Stats {
        #[command(subcommand)]
        action: Option<StatsCommand>,
    },

    /// Server and dependency health, at a glance
    Status,

    /// Full-screen interactive browser
    Tui(TuiArgs),

    /// Run and manage the OVIS backend
    Server {
        #[command(subcommand)]
        action: ServerCommand,
    },

    /// Inspect and edit CLI configuration
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },

    /// Generate shell completions
    Completions {
        #[arg(value_name = "SHELL")]
        shell: clap_complete::Shell,
    },

    /// Deferred: the pruning engine is out of scope for this redesign
    Prune,

    /// Print connector names, one per line (used by shell completions)
    #[command(name = "__connector-names", hide = true)]
    ConnectorNames,
}

// ---------------------------------------------------------------------------
// page
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug, Clone)]
pub enum PageCommand {
    /// List pages
    #[command(alias = "ls")]
    List(PageListArgs),

    /// Show one page's metadata
    #[command(alias = "show", alias = "inspect")]
    View {
        /// Document id (a URL) or an @N handle from the last list
        #[arg(value_name = "ID|@N")]
        id: String,
    },

    /// Print a page's reconstructed text
    Text {
        #[arg(value_name = "ID|@N")]
        id: String,
        /// Write to this file instead of the pager
        #[arg(short = 'O', long, value_name = "PATH")]
        output: Option<String>,
    },

    /// List a page's indexed chunks
    Chunks {
        #[arg(value_name = "ID|@N")]
        id: String,
        /// Maximum chunks to fetch
        #[arg(short = 'n', long, default_value_t = 100)]
        limit: i64,
        /// Resume after this chunk index
        #[arg(long, value_name = "N")]
        after: Option<i64>,
        /// Include full chunk text rather than a blurb
        #[arg(long)]
        full: bool,
    },

    /// Open a page's link in the browser
    Open {
        #[arg(value_name = "ID|@N")]
        id: String,
    },

    /// Change a page's title, boost, visibility or metadata
    Edit {
        #[arg(value_name = "ID|@N")]
        id: String,

        /// New title (semantic id)
        #[arg(long, value_name = "TEXT")]
        title: Option<String>,

        /// Relevance boost
        #[arg(long, value_name = "N", allow_negative_numbers = true)]
        boost: Option<i32>,

        /// Hide from search results
        #[arg(long, conflicts_with = "unhide")]
        hide: bool,

        /// Un-hide
        #[arg(long)]
        unhide: bool,

        /// Shallow-merge a metadata key, e.g. --meta owner=arvind (repeatable)
        #[arg(long, value_name = "KEY=VALUE")]
        meta: Vec<String>,
    },

    /// Delete pages from Postgres and the search index
    #[command(alias = "rm")]
    Delete {
        /// Document ids, @N handles, or `-` to read ids from stdin
        #[arg(value_name = "ID|@N")]
        ids: Vec<String>,

        /// Read ids from a file, one per line
        #[arg(long, value_name = "PATH")]
        from_file: Option<String>,
    },

    /// Content search (alias of the top-level `ovis search`)
    Search(SearchArgs),
}

#[derive(Args, Debug, Clone, Default)]
pub struct PageListArgs {
    /// Filter titles and URLs
    #[arg(value_name = "QUERY")]
    pub query: Option<String>,

    /// Connector id or name
    #[arg(short = 'c', long, value_name = "ID|NAME")]
    pub connector: Option<String>,

    /// Source type: web, github, wikipedia, ingestion_api
    #[arg(short = 's', long, value_name = "SOURCE")]
    pub source: Option<String>,

    /// Only pages with no chunks in the index
    #[arg(long, conflicts_with_all = ["heavy", "chunks"])]
    pub stubs: bool,

    /// Only pages with more than ten chunks
    #[arg(long, conflicts_with = "chunks")]
    pub heavy: bool,

    /// Chunk-count range, e.g. 1..5, ..0, 20..
    #[arg(long, value_name = "MIN..MAX")]
    pub chunks: Option<String>,

    /// Only hidden pages
    #[arg(long, conflicts_with = "visible")]
    pub hidden: bool,

    /// Only visible pages
    #[arg(long)]
    pub visible: bool,

    /// Only pages updated since then: 2h, 3d, or 2026-07-01
    #[arg(long, value_name = "WHEN")]
    pub since: Option<String>,

    /// Only pages updated before then
    #[arg(long, value_name = "WHEN")]
    pub until: Option<String>,

    /// updated|chunks|id|boost, optionally :asc or :desc
    #[arg(long, value_name = "FIELD[:DIR]")]
    pub sort: Option<String>,

    /// Rows to fetch [default: fits the terminal, minimum 20]
    #[arg(short = 'n', long, value_name = "N")]
    pub limit: Option<i64>,

    /// Page number for offset pagination
    #[arg(long, value_name = "N", conflicts_with_all = ["cursor", "all"])]
    pub page: Option<i64>,

    /// Keyset cursor from a previous response
    #[arg(long, value_name = "TOKEN", conflicts_with = "all")]
    pub cursor: Option<String>,

    /// Stream every matching row (SSE keyset; use with -o ndjson)
    #[arg(long)]
    pub all: bool,

    /// Choose one result interactively and show it
    #[arg(long, conflicts_with = "all")]
    pub pick: bool,
}

#[derive(Args, Debug, Clone, Default)]
pub struct SearchArgs {
    #[arg(value_name = "QUERY")]
    pub query: Vec<String>,

    /// keyword | semantic | hybrid. On this deployment the vector modes degrade
    /// to keyword and say so — see `ovis status`.
    #[arg(long, default_value = "keyword", value_name = "MODE")]
    pub mode: String,

    /// Connector id or name (best-effort: applied after ranking)
    #[arg(short = 'c', long, value_name = "ID|NAME")]
    pub connector: Option<String>,

    /// Source type
    #[arg(short = 's', long, value_name = "SOURCE")]
    pub source: Option<String>,

    /// Include hidden documents
    #[arg(long)]
    pub include_hidden: bool,

    /// Maximum hits
    #[arg(short = 'n', long, value_name = "N")]
    pub limit: Option<i64>,

    /// Skip this many hits
    #[arg(long, value_name = "N")]
    pub offset: Option<i64>,
}

// ---------------------------------------------------------------------------
// connector
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug, Clone)]
pub enum ConnectorCommand {
    /// List connectors with real status and document counts
    #[command(alias = "ls")]
    List {
        /// Filter by name substring
        #[arg(value_name = "QUERY")]
        query: Option<String>,

        /// Filter by status: ACTIVE, PAUSED, INITIAL_INDEXING, …
        #[arg(long, value_name = "STATUS")]
        status: Option<String>,

        /// Only connectors parked by the resilience cron
        #[arg(long)]
        parked: bool,

        /// Filter by source type
        #[arg(short = 's', long, value_name = "SOURCE")]
        source: Option<String>,

        /// name|docs|status|source
        #[arg(long, default_value = "docs", value_name = "FIELD")]
        sort: String,
    },

    /// Show one connector in detail
    #[command(alias = "show")]
    View {
        #[arg(value_name = "CC_PAIR|NAME")]
        connector: String,

        /// Add a daily documents-added history, e.g. 7d
        #[arg(long, value_name = "DAYS")]
        history: Option<String>,
    },

    /// List a connector's documents
    Docs {
        #[arg(value_name = "CC_PAIR|NAME")]
        connector: String,
        #[arg(short = 'n', long, value_name = "N")]
        limit: Option<i64>,
        #[arg(long, value_name = "N")]
        page: Option<i64>,
    },

    /// Index attempts, for one connector or globally
    Attempts {
        #[arg(value_name = "CC_PAIR|NAME")]
        connector: Option<String>,
        /// Comma-separated statuses, e.g. IN_PROGRESS,FAILED
        #[arg(long, value_name = "LIST")]
        status: Option<String>,
        #[arg(short = 'n', long, value_name = "N")]
        limit: Option<i64>,
        #[arg(long, value_name = "N")]
        page: Option<i64>,
    },

    /// Documents that failed to index for this connector
    Errors {
        #[arg(value_name = "CC_PAIR|NAME")]
        connector: String,
        #[arg(short = 'n', long, value_name = "N")]
        limit: Option<i64>,
        #[arg(long, value_name = "N")]
        page: Option<i64>,
        /// Only failures nobody has resolved
        #[arg(long)]
        unresolved: bool,
    },

    /// Pause indexing
    Pause {
        #[arg(value_name = "CC_PAIR|NAME", required = true)]
        connectors: Vec<String>,
    },

    /// Resume indexing
    Resume {
        #[arg(value_name = "CC_PAIR|NAME", required = true)]
        connectors: Vec<String>,
    },

    /// Trigger one crawl of this connector
    Run {
        #[arg(value_name = "CC_PAIR|NAME")]
        connector: String,

        /// Re-crawl from the beginning rather than incrementally
        #[arg(long)]
        from_beginning: bool,

        /// Proceed even though the resilience cron parked this cc-pair
        #[arg(long)]
        acknowledge_parked: bool,
    },

    /// Ask Onyx to prune this connector's deleted documents
    Prune {
        #[arg(value_name = "CC_PAIR|NAME")]
        connector: String,
    },

    /// Delete a connector and everything it indexed
    #[command(alias = "rm")]
    Delete {
        #[arg(value_name = "CC_PAIR|NAME")]
        connector: String,

        /// The exact connector name, echoed back as confirmation
        #[arg(long, value_name = "NAME")]
        confirm_name: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// stats / tui / server / config
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug, Clone)]
pub enum StatsCommand {
    /// Documents, chunks, connectors, index and crawl health
    Overview,
    /// The connectors holding the most documents
    Connectors {
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: i64,
        /// docs | recent
        #[arg(long, default_value = "docs")]
        by: String,
    },
    /// Documents added over time
    Timeline {
        /// 24h | 7d | 30d
        #[arg(long, default_value = "24h")]
        window: String,
        /// 1h | 1d
        #[arg(long)]
        bucket: Option<String>,
    },
    /// Documents and chunks per source type
    Sources,
}

#[derive(Args, Debug, Clone, Default)]
pub struct TuiArgs {
    /// Start scoped to this connector
    #[arg(short = 'c', long, value_name = "ID|NAME")]
    pub connector: Option<String>,

    /// Start with this filter applied
    #[arg(long, value_name = "TEXT")]
    pub query: Option<String>,

    /// pages | connectors | activity
    #[arg(long, value_name = "SCREEN")]
    pub screen: Option<String>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ServerCommand {
    /// Run the OVIS backend
    Start {
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,
        #[arg(long, value_name = "HOST")]
        host: Option<String>,
        /// Run in the background
        #[arg(short = 'd', long)]
        detach: bool,
        /// Backend config file (TOML)
        #[arg(long, value_name = "PATH")]
        config: Option<String>,
    },

    /// Stop a backend started with --detach
    Stop {
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,
    },

    /// Stop and start again
    Restart {
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,
        #[arg(long, value_name = "HOST")]
        host: Option<String>,
        #[arg(short = 'd', long)]
        detach: bool,
        #[arg(long, value_name = "PATH")]
        config: Option<String>,
    },

    /// Whether a backend is answering, and how healthily
    Status {
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,
    },

    /// Mint an Onyx token so connector actions work, and store it
    SetupOnyxKey {
        /// Onyx base URL
        #[arg(long, value_name = "URL")]
        onyx_url: Option<String>,
        /// Onyx admin email
        #[arg(long, value_name = "EMAIL")]
        email: Option<String>,
        /// Name to give the token in Onyx
        #[arg(long, default_value = "ovis", value_name = "NAME")]
        name: String,
        /// Print the token instead of writing it to the config file
        #[arg(long)]
        print_only: bool,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum ConfigCommand {
    /// Write an annotated config file
    Init {
        /// Overwrite an existing file
        #[arg(long)]
        force: bool,
    },
    /// Print the effective configuration
    Show {
        /// Say where each value came from
        #[arg(long)]
        origin: bool,
    },
    /// Set one key, e.g. profiles.homelab.server http://gamma:8080
    Set {
        #[arg(value_name = "KEY")]
        key: String,
        #[arg(value_name = "VALUE")]
        value: String,
    },
    /// Print the config file path
    Path,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_tree_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn global_flags_are_accepted_after_a_subcommand() {
        // The headline defect S1: `ovis page list --format json` used to be a
        // parse error, contradicting the spec's own examples.
        let cli = Cli::try_parse_from(["ovis", "page", "list", "--format", "json"]).unwrap();
        assert_eq!(cli.globals.format, Some(Format::Json));

        let cli =
            Cli::try_parse_from(["ovis", "page", "view", "https://x/y", "-o", "yaml"]).unwrap();
        assert_eq!(cli.globals.format, Some(Format::Yaml));

        // …and still before it.
        let cli = Cli::try_parse_from(["ovis", "-o", "csv", "connector", "list"]).unwrap();
        assert_eq!(cli.globals.format, Some(Format::Csv));
    }

    #[test]
    fn every_global_flag_reaches_the_deepest_subcommand() {
        let cli = Cli::try_parse_from([
            "ovis",
            "connector",
            "run",
            "tildes",
            "--server",
            "http://gamma:8080",
            "--token",
            "t",
            "--color",
            "never",
            "--profile",
            "homelab",
            "--no-input",
            "-y",
            "-q",
            "-vv",
            "--wide",
            "--no-headers",
            "--columns",
            "name",
        ])
        .unwrap();
        assert_eq!(cli.globals.server.as_deref(), Some("http://gamma:8080"));
        assert_eq!(cli.globals.token.as_deref(), Some("t"));
        assert_eq!(cli.globals.color, Some(crate::output::ColorChoice::Never));
        assert_eq!(cli.globals.profile.as_deref(), Some("homelab"));
        assert!(cli.globals.no_input && cli.globals.yes && cli.globals.quiet);
        assert_eq!(cli.globals.verbose, 2);
        assert!(cli.globals.wide && cli.globals.no_headers);
        assert_eq!(cli.globals.columns.as_deref(), Some("name"));
    }

    #[test]
    fn the_short_aliases_from_the_navigation_guide_work() {
        // `ovis p ls kant` is the documented hot path.
        let cli = Cli::try_parse_from(["ovis", "p", "ls", "kant"]).unwrap();
        match cli.command {
            Command::Page {
                action: PageCommand::List(args),
            } => assert_eq!(args.query.as_deref(), Some("kant")),
            other => panic!("expected page list, got {other:?}"),
        }
        assert!(Cli::try_parse_from(["ovis", "c", "ls"]).is_ok());
        assert!(Cli::try_parse_from(["ovis", "pages", "show", "x"]).is_ok());
        assert!(Cli::try_parse_from(["ovis", "page", "inspect", "x"]).is_ok());
        assert!(Cli::try_parse_from(["ovis", "p", "rm", "@1"]).is_ok());
    }

    #[test]
    fn removed_flags_are_gone_rather_than_silently_ignored() {
        // These carried a production DSN and a password.
        for flag in [
            "--db-dsn",
            "--postgres-url",
            "--opensearch-url",
            "--search-engine",
            "--auto-detect",
            "--onyx",
            "--force",
        ] {
            assert!(
                Cli::try_parse_from(["ovis", "page", "list", flag, "x"]).is_err(),
                "{flag} should no longer parse"
            );
        }
    }

    #[test]
    fn delete_takes_several_ids_and_handles() {
        let cli =
            Cli::try_parse_from(["ovis", "page", "delete", "@1", "@2", "https://x/y"]).unwrap();
        match cli.command {
            Command::Page {
                action: PageCommand::Delete { ids, .. },
            } => assert_eq!(ids, vec!["@1", "@2", "https://x/y"]),
            other => panic!("expected page delete, got {other:?}"),
        }
    }

    #[test]
    fn contradictory_list_filters_are_rejected_at_parse_time() {
        assert!(Cli::try_parse_from(["ovis", "p", "ls", "--stubs", "--heavy"]).is_err());
        assert!(Cli::try_parse_from(["ovis", "p", "ls", "--hidden", "--visible"]).is_err());
        assert!(Cli::try_parse_from(["ovis", "p", "ls", "--all", "--page", "2"]).is_err());
        assert!(Cli::try_parse_from(["ovis", "p", "ls", "--all", "--cursor", "x"]).is_err());
        assert!(Cli::try_parse_from(["ovis", "p", "edit", "x", "--hide", "--unhide"]).is_err());
    }

    #[test]
    fn a_negative_boost_parses_as_a_value_not_as_a_flag() {
        let cli = Cli::try_parse_from(["ovis", "page", "edit", "x", "--boost", "-2"]).unwrap();
        match cli.command {
            Command::Page {
                action: PageCommand::Edit { boost, .. },
            } => assert_eq!(boost, Some(-2)),
            other => panic!("expected page edit, got {other:?}"),
        }
    }

    #[test]
    fn a_multi_word_search_query_does_not_need_quoting() {
        let cli = Cli::try_parse_from(["ovis", "search", "kant", "aesthetics"]).unwrap();
        match cli.command {
            Command::Search(args) => assert_eq!(args.query, vec!["kant", "aesthetics"]),
            other => panic!("expected search, got {other:?}"),
        }
    }

    #[test]
    fn there_is_no_bulk_crawl_trigger_anywhere() {
        // Landmine #5: never blanket-trigger indexing across ACTIVE connectors.
        // `run` takes exactly one connector, unlike pause/resume.
        assert!(Cli::try_parse_from(["ovis", "connector", "run", "a", "b"]).is_err());
        assert!(Cli::try_parse_from(["ovis", "connector", "pause", "a", "b"]).is_ok());
        assert!(Cli::try_parse_from(["ovis", "connector", "run-all"]).is_err());
    }
}
