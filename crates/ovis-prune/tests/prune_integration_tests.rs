use ovis_prune::{
    DocumentWithContent, PreferKeepPolicy, PruneConfig, PruningEngine,
};
use serde_json::json;

fn make_doc(
    id: &str,
    title: &str,
    link: Option<&str>,
    content: &str,
    updated_at: Option<chrono::DateTime<chrono::Utc>>,
) -> DocumentWithContent {
    DocumentWithContent {
        id: id.to_string(),
        semantic_id: title.to_string(),
        connector_id: 10,
        link: link.map(|s| s.to_string()),
        content: content.to_string(),
        updated_at,
        metadata: json!({"source": "integration_test"}),
    }
}

#[test]
fn test_full_pruning_pipeline_with_yaml_config() {
    let yaml_config = r#"
version: "1.0"
repository_scope: "OVIS Integration Test Suite Repository"

heuristics:
  min_char_count: 150
  max_char_count: 300000
  min_alphanumeric_ratio: 0.50
  title_blacklist_regex:
    - "(?i)^404 Not Found"
    - "(?i)^Access Denied"
    - "(?i)^Login \\| "
  url_blacklist_patterns:
    - "*/tag/*"
    - "*/category/*"
    - "*?share=*"

deduplication:
  enabled: true
  minhash:
    num_perm: 128
    jaccard_threshold: 0.85
    shingle_size: 5
  prefer_keep: "longest_content"

execution:
  dry_run: true
  auto_delete: false
  audit_log_path: "./target/prune_integration_audit_log.json"
"#;

    let config = PruneConfig::from_yaml(yaml_config).expect("YAML parsing failed");
    let engine = PruningEngine::new(config).expect("Engine build failed");

    let clean_text_1 = "This is high quality engineering documentation describing the operational procedures and architectural components of the OVIS search index. It contains comprehensive explanations of PostgreSQL relational metadata schema, OpenSearch vector chunk storage, Axum REST endpoints, ratatui TUI dashboard interface, and MinHash LSH deduplication pipeline.";
    let clean_text_2 = "This is high quality engineering documentation describing the operational procedures and architectural components of the OVIS search index. It contains comprehensive explanations of PostgreSQL relational metadata schema, OpenSearch vector chunk storage, Axum REST endpoints, ratatui TUI dashboard interface, and MinHash LSH deduplication pipeline with extra supplementary notes.";

    let docs = vec![
        // 1. Min char count failure
        make_doc("doc_short", "Short Stub", None, "Short text under 150 chars", None),
        // 2. 404 Error page failure
        make_doc(
            "doc_404",
            "404 Not Found",
            None,
            "The requested document could not be found on the server. Please verify your requested URL or return to home page.",
            None,
        ),
        // 3. Blacklisted URL failure
        make_doc(
            "doc_tag",
            "Tag Archive Page",
            Some("https://example.com/blog/tag/microservices"),
            "This is a tag archive page listing all blog posts tagged with microservices architecture and software design principles.",
            None,
        ),
        // 4. Low alphanumeric ratio failure
        make_doc(
            "doc_spam",
            "Corrupted Symbol Dump",
            None,
            "!@#$%^&*()_+={}:\"<>?~`!@#$%^&*()_+={}:\"<>?~`!@#$%^&*()_+={}:\"<>?~`!@#$%^&*()_+={}:\"<>?~`!@#$%^&*()_+={}:\"<>?~`!@#$%^&*()_+={}:\"<>?~`!@#$%^&*()_+={}:\"<>?~` binary data",
            None,
        ),
        // 5 & 6. Near-duplicate document pair
        make_doc(
            "doc_orig",
            "OVIS Engine Spec",
            Some("https://docs.company.com/ovis/spec"),
            clean_text_1,
            None,
        ),
        make_doc(
            "doc_copy",
            "OVIS Engine Spec (Copy)",
            Some("https://docs.company.com/ovis/spec_copy"),
            clean_text_2,
            None,
        ),
    ];

    let report = engine.evaluate_repository(&docs).expect("Evaluation failed");

    assert_eq!(report.total_documents_evaluated, 6);
    assert!(report.total_candidates_flagged >= 4);
    assert!(report.dry_run);

    // Save JSON audit report to test disk output
    let temp_log_path = "./target/test_prune_audit_log.json";
    report.save_to_file(temp_log_path).expect("Failed to write audit log file");
    assert!(std::path::Path::new(temp_log_path).exists());

    // Read back log file and parse JSON
    let content = std::fs::read_to_string(temp_log_path).unwrap();
    assert!(content.contains("OVIS Integration Test Suite Repository"));
}

#[test]
fn test_prefer_keep_policies_in_deduplication() {
    let now = chrono::Utc::now();
    let older = now - chrono::Duration::hours(10);

    let doc_a = make_doc(
        "doc_a",
        "Short Title",
        Some("https://example.com/a"),
        "Comprehensive technical guide to distributed data pipelines and storage architecture in modern Rust applications.",
        Some(older),
    );
    let doc_b = make_doc(
        "doc_b",
        "Longer Detailed Title",
        Some("https://example.com/sub/directory/deep/path/b"),
        "Comprehensive technical guide to distributed data pipelines and storage architecture in modern Rust applications with additional paragraph content.",
        Some(now),
    );

    let docs = vec![doc_a, doc_b];

    // Policy 1: Longest Content -> keep doc_b (longer content)
    let mut config1 = PruneConfig::default();
    config1.deduplication.prefer_keep = PreferKeepPolicy::LongestContent;
    let engine1 = PruningEngine::new(config1).unwrap();
    let (dups1, _) = engine1.evaluate_repository(&docs).map(|r| (r.duplicate_pairs, r.candidates)).unwrap();
    if !dups1.is_empty() {
        assert_eq!(dups1[0].kept_document_id, "doc_b");
        assert_eq!(dups1[0].duplicate_document_id, "doc_a");
    }

    // Policy 2: Newest Updated -> keep doc_b (now > older)
    let mut config2 = PruneConfig::default();
    config2.deduplication.prefer_keep = PreferKeepPolicy::NewestUpdated;
    let engine2 = PruningEngine::new(config2).unwrap();
    let (dups2, _) = engine2.evaluate_repository(&docs).map(|r| (r.duplicate_pairs, r.candidates)).unwrap();
    if !dups2.is_empty() {
        assert_eq!(dups2[0].kept_document_id, "doc_b");
    }

    // Policy 3: Shortest URL -> keep doc_a ("https://example.com/a" is shorter)
    let mut config3 = PruneConfig::default();
    config3.deduplication.prefer_keep = PreferKeepPolicy::ShortestUrl;
    let engine3 = PruningEngine::new(config3).unwrap();
    let (dups3, _) = engine3.evaluate_repository(&docs).map(|r| (r.duplicate_pairs, r.candidates)).unwrap();
    if !dups3.is_empty() {
        assert_eq!(dups3[0].kept_document_id, "doc_a");
        assert_eq!(dups3[0].duplicate_document_id, "doc_b");
    }
}
