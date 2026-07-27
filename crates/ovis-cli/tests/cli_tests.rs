use clap::Parser;
use ovis_cli::cli::{Cli, Commands, ConnectorCommands, OutputFormat, PageCommands, PruneSubcommands};
use ovis_cli::formatters::Formatter;
use ovis_cli::tui::app::{ActivePane, App};
use ovis_cli::models::{ChunkRecord, DocumentRecord};
use serde_json::json;

fn create_test_docs() -> Vec<DocumentRecord> {
    vec![
        DocumentRecord {
            id: "https://docs.onyx.app/web".to_string(),
            from_beginning: Some(true),
            semantic_id: "Web Connector Docs".to_string(),
            link: Some("https://docs.onyx.app/web".to_string()),
            doc_updated_at: None,
            primary_owners: Some(vec!["team@onyx.app".to_string()]),
            secondary_owners: None,
            metadata: json!({"source": "web", "connector_id": 2, "chunks": 4}),
        },
        DocumentRecord {
            id: "https://github.com/onyx-dot-app".to_string(),
            from_beginning: Some(true),
            semantic_id: "Onyx GitHub Repo".to_string(),
            link: Some("https://github.com/onyx-dot-app".to_string()),
            doc_updated_at: None,
            primary_owners: Some(vec!["devs@onyx.app".to_string()]),
            secondary_owners: None,
            metadata: json!({"source": "github", "connector_id": 1, "chunks": 12}),
        },
    ]
}

#[test]
fn test_cli_parsing_all_subcommands() {
    // 1. Server subcommand
    let cli = Cli::try_parse_from(["ovis", "server", "--port", "9090", "--host", "127.0.0.1"]).unwrap();
    if let Commands::Server { port, host, .. } = cli.command {
        assert_eq!(port, 9090);
        assert_eq!(host, "127.0.0.1");
    } else {
        panic!("Expected Server subcommand");
    }

    // 1b. Server subcommands (start/stop/restart/status with --auto-detect)
    let cli_start = Cli::try_parse_from(["ovis", "server", "start", "--port", "8080", "--detach", "--auto-detect"]).unwrap();
    if let Commands::Server { action: Some(ovis_cli::cli::ServerSubcommands::Start { port, detach, auto_detect, .. }), .. } = cli_start.command {
        assert_eq!(port, 8080);
        assert!(detach);
        assert!(auto_detect);
    } else {
        panic!("Expected Server Start subcommand");
    }

    let cli_stop = Cli::try_parse_from(["ovis", "server", "stop", "--port", "8080"]).unwrap();
    if let Commands::Server { action: Some(ovis_cli::cli::ServerSubcommands::Stop { port }), .. } = cli_stop.command {
        assert_eq!(port, 8080);
    } else {
        panic!("Expected Server Stop subcommand");
    }

    let cli_status = Cli::try_parse_from(["ovis", "server", "status", "--port", "8080"]).unwrap();
    if let Commands::Server { action: Some(ovis_cli::cli::ServerSubcommands::Status { port }), .. } = cli_status.command {
        assert_eq!(port, 8080);
    } else {
        panic!("Expected Server Status subcommand");
    }

    // 2. Connector list subcommand
    let cli = Cli::try_parse_from(["ovis", "connector", "list"]).unwrap();
    if let Commands::Connector { action: ConnectorCommands::List } = cli.command {
        // Ok
    } else {
        panic!("Expected Connector List subcommand");
    }

    // 3. Page list subcommand
    let cli = Cli::try_parse_from(["ovis", "page", "list", "--source", "web", "--limit", "20"]).unwrap();
    if let Commands::Page { action: PageCommands::List { source, limit, .. } } = cli.command {
        assert_eq!(source, Some("web".to_string()));
        assert_eq!(limit, 20);
    } else {
        panic!("Expected Page List subcommand");
    }

    // 4. Page inspect subcommand
    let cli = Cli::try_parse_from(["ovis", "page", "inspect", "doc_123", "--raw"]).unwrap();
    if let Commands::Page { action: PageCommands::Inspect { id, raw } } = cli.command {
        assert_eq!(id, "doc_123");
        assert!(raw);
    } else {
        panic!("Expected Page Inspect subcommand");
    }

    // 5. Page delete subcommand
    let cli = Cli::try_parse_from(["ovis", "page", "delete", "doc_123", "--yes"]).unwrap();
    if let Commands::Page { action: PageCommands::Delete { id, yes } } = cli.command {
        assert_eq!(id, "doc_123");
        assert!(yes);
    } else {
        panic!("Expected Page Delete subcommand");
    }

    // 6. Prune run subcommand
    let cli = Cli::try_parse_from(["ovis", "prune", "run", "--config", "my_prune.yaml", "--dry-run"]).unwrap();
    if let Commands::Prune { action: Some(PruneSubcommands::Run { config, dry_run, .. }), .. } = cli.command {
        assert_eq!(config, "my_prune.yaml");
        assert!(dry_run);
    } else {
        panic!("Expected Prune Run subcommand");
    }

    // 7. TUI subcommand
    let cli = Cli::try_parse_from(["ovis", "tui", "--search", "onyx"]).unwrap();
    if let Commands::Tui { search, .. } = cli.command {
        assert_eq!(search, Some("onyx".to_string()));
    } else {
        panic!("Expected TUI subcommand");
    }
}

#[test]
fn test_formatter_table_and_json() {
    let docs = create_test_docs();

    // Test JSON output
    let mut buf = Vec::new();
    Formatter::print_documents(&mut buf, &docs, OutputFormat::Json).unwrap();
    let json_str = String::from_utf8(buf).unwrap();
    assert!(json_str.contains("\"total\": 2"));
    assert!(json_str.contains("https://docs.onyx.app/web"));

    // Test Table output
    let mut buf = Vec::new();
    Formatter::print_documents(&mut buf, &docs, OutputFormat::Table).unwrap();
    let table_str = String::from_utf8(buf).unwrap();
    assert!(table_str.contains("DOCUMENT ID"));
    assert!(table_str.contains("Web Connector Docs"));

    // Test YAML output
    let mut buf = Vec::new();
    Formatter::print_documents(&mut buf, &docs, OutputFormat::Yaml).unwrap();
    let yaml_str = String::from_utf8(buf).unwrap();
    assert!(yaml_str.contains("total: 2"));
}

#[test]
fn test_tui_app_state_logic() {
    let docs = create_test_docs();
    let chunks = vec![ChunkRecord {
        chunk_id: 0,
        document_id: "https://docs.onyx.app/web".to_string(),
        content: "Sample text content".to_string(),
        title: Some("Title".to_string()),
        source_type: "web".to_string(),
        metadata: json!({}),
        embeddings: None,
    }];

    let mut app = App::new(docs, chunks);
    assert_eq!(app.filtered_documents.len(), 2);
    assert_eq!(app.active_pane, ActivePane::LeftList);

    // Test selection navigation
    app.select_next();
    assert_eq!(app.selected_index, 1);
    assert_eq!(app.selected_document().unwrap().semantic_id, "Onyx GitHub Repo");

    // Test search filter
    app.search_query = "github".to_string();
    app.apply_filter();
    assert_eq!(app.filtered_documents.len(), 1);
    assert_eq!(app.selected_document().unwrap().semantic_id, "Onyx GitHub Repo");

    // Test pane toggle
    app.toggle_pane();
    assert_eq!(app.active_pane, ActivePane::RightInspector);
    app.toggle_pane();
    assert_eq!(app.active_pane, ActivePane::LeftList);

    // Test deletion
    app.delete_selected();
    assert_eq!(app.all_documents.len(), 1);
}
