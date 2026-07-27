use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

/// Output format options for CLI execution
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Table,
    Json,
    Yaml,
    Csv,
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputFormat::Table => write!(f, "table"),
            OutputFormat::Json => write!(f, "json"),
            OutputFormat::Yaml => write!(f, "yaml"),
            OutputFormat::Csv => write!(f, "csv"),
        }
    }
}

/// Onyx Visibility CLI & Administration Tool
#[derive(Parser, Debug, Clone)]
#[command(
    name = "ovis",
    version,
    about = "Onyx Visibility CLI & Administration Tool"
)]
pub struct Cli {
    /// Onyx PostgreSQL DSN connection string
    #[arg(long, alias = "postgres-url", env = "POSTGRES_URL")]
    pub db_dsn: Option<String>,

    /// OpenSearch URL
    #[arg(long, alias = "search-url", env = "SEARCH_ENGINE_URL")]
    pub opensearch_url: Option<String>,

    /// Search engine type
    #[arg(long, default_value = "opensearch")]
    pub search_engine: String,

    /// Output format: table, json, yaml, csv
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,

    /// Skip confirmation prompts
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Force execution without safety checks
    #[arg(short = 'f', long)]
    pub force: bool,

    /// Auto-detect live Onyx stack (PostgreSQL & OpenSearch on gamma/local)
    #[arg(long, alias = "onyx", alias = "auto")]
    pub auto_detect: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    /// Start, stop, restart, or check status of OVIS Web & Frontend SPA server
    Server {
        #[command(subcommand)]
        action: Option<ServerSubcommands>,

        #[arg(long, default_value_t = 8080)]
        port: u16,

        #[arg(long, default_value = "0.0.0.0")]
        host: String,

        /// Run server in detached background daemon mode
        #[arg(short = 'd', long)]
        detach: bool,

        /// Deprecated and now a no-op: connection details come from --db-dsn /
        /// --opensearch-url or the environment. Nothing is probed and no host or
        /// credential is compiled in.
        #[arg(long, alias = "onyx")]
        auto_detect: bool,
    },

    /// Connector management subcommands
    Connector {
        #[command(subcommand)]
        action: ConnectorCommands,
    },

    /// Page management subcommands
    Page {
        #[command(subcommand)]
        action: PageCommands,
    },

    /// Run relevance and duplicate pruning pipeline
    Prune {
        #[command(subcommand)]
        action: Option<PruneSubcommands>,

        /// Path to prune.yaml configuration file
        #[arg(short, long, default_value = "prune.yaml")]
        config: String,

        /// Dry run mode preview without deleting documents
        #[arg(long)]
        dry_run: bool,

        /// Force deletion without confirmation
        #[arg(long)]
        force: bool,
    },

    /// Launch interactive terminal UI dashboard
    Tui {
        /// Pre-filter by connector ID
        #[arg(long)]
        connector: Option<i32>,

        /// Pre-fill search filter term
        #[arg(long)]
        search: Option<String>,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum ServerSubcommands {
    /// Start frontend SPA & backend server
    Start {
        #[arg(long, default_value_t = 8080)]
        port: u16,

        #[arg(long, default_value = "0.0.0.0")]
        host: String,

        /// Run in background daemon mode
        #[arg(short = 'd', long)]
        detach: bool,

        /// Deprecated and now a no-op: connection details come from --db-dsn /
        /// --opensearch-url or the environment. Nothing is probed and no host or
        /// credential is compiled in.
        #[arg(long, alias = "onyx")]
        auto_detect: bool,
    },

    /// Stop running server process
    Stop {
        #[arg(long, default_value_t = 8080)]
        port: u16,
    },

    /// Restart server process
    Restart {
        #[arg(long, default_value_t = 8080)]
        port: u16,

        #[arg(long, default_value = "0.0.0.0")]
        host: String,

        /// Run in background daemon mode
        #[arg(short = 'd', long)]
        detach: bool,

        /// Deprecated and now a no-op: connection details come from --db-dsn /
        /// --opensearch-url or the environment. Nothing is probed and no host or
        /// credential is compiled in.
        #[arg(long, alias = "onyx")]
        auto_detect: bool,
    },

    /// Check server status and health
    Status {
        #[arg(long, default_value_t = 8080)]
        port: u16,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum ConnectorCommands {
    /// List registered Onyx connectors and page statistics
    List,
}

#[derive(Subcommand, Debug, Clone)]
pub enum PageCommands {
    /// List and filter crawled pages
    List {
        /// Filter by connector ID
        #[arg(long)]
        connector: Option<i32>,

        /// Filter by source type (e.g. web, github, file)
        #[arg(long)]
        source: Option<String>,

        /// Search query matching semantic ID or URL
        #[arg(long)]
        search: Option<String>,

        /// Maximum number of documents to return
        #[arg(long, default_value_t = 50)]
        limit: usize,

        /// Offset for document list pagination
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },

    /// Inspect raw page content, metadata, & vector chunks
    Inspect {
        /// Target document ID or URL
        id: String,

        /// Output raw document text content
        #[arg(long)]
        raw: bool,
    },

    /// Edit page title or metadata tags
    Edit {
        /// Target document ID or URL
        id: String,

        /// Updated document title / semantic ID
        #[arg(long)]
        title: Option<String>,

        /// Comma-separated list of metadata tags
        #[arg(long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
    },

    /// Delete a specific page across Postgres & Search index
    Delete {
        /// Target document ID or URL
        id: String,

        /// Skip deletion confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum PruneSubcommands {
    /// Run pruning pipeline
    Run {
        /// Path to prune.yaml configuration file
        #[arg(short, long, default_value = "prune.yaml")]
        config: String,

        /// Dry run mode preview without deleting documents
        #[arg(long)]
        dry_run: bool,

        /// Force execution
        #[arg(long)]
        force: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_parse_page_list() {
        let args = vec!["ovis", "page", "list", "--connector", "2", "--limit", "10"];
        let cli = Cli::try_parse_from(args).expect("Failed to parse page list args");
        if let Commands::Page { action: PageCommands::List { connector, limit, .. } } = cli.command {
            assert_eq!(connector, Some(2));
            assert_eq!(limit, 10);
        } else {
            panic!("Expected Page::List command");
        }
    }

    #[test]
    fn test_cli_parse_page_delete() {
        let args = vec!["ovis", "--format", "json", "page", "delete", "https://docs.onyx.app/web", "--yes"];
        let cli = Cli::try_parse_from(args).expect("Failed to parse page delete args");
        assert_eq!(cli.format, OutputFormat::Json);
        if let Commands::Page { action: PageCommands::Delete { id, yes } } = cli.command {
            assert_eq!(id, "https://docs.onyx.app/web");
            assert!(yes);
        } else {
            panic!("Expected Page::Delete command");
        }
    }

    #[test]
    fn test_cli_parse_prune_run() {
        let args = vec!["ovis", "prune", "run", "--config", "custom_prune.yaml", "--dry-run"];
        let cli = Cli::try_parse_from(args).expect("Failed to parse prune run args");
        if let Commands::Prune { action: Some(PruneSubcommands::Run { config, dry_run, .. }), .. } = cli.command {
            assert_eq!(config, "custom_prune.yaml");
            assert!(dry_run);
        } else {
            panic!("Expected Prune::Run command");
        }
    }

    #[test]
    fn test_cli_parse_tui() {
        let args = vec!["ovis", "tui", "--connector", "1"];
        let cli = Cli::try_parse_from(args).expect("Failed to parse tui args");
        if let Commands::Tui { connector, .. } = cli.command {
            assert_eq!(connector, Some(1));
        } else {
            panic!("Expected Tui command");
        }
    }
}
