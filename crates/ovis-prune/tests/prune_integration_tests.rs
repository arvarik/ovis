//! Crate-level integration: YAML config in, detection over an in-memory
//! corpus out. (The old tests here exercised the self-contained
//! `PruningEngine`, which the 2026 rework replaced with backend-fed
//! detectors; the kept core — config round-trip + MinHash dedup — is what
//! these cover.)

use ovis_prune::{
    DocumentWithContent, MinHashDedupEngine, PreferKeepPolicy, PruneConfig,
};

fn make_doc(id: &str, link: Option<&str>, content: &str) -> DocumentWithContent {
    DocumentWithContent {
        id: id.to_string(),
        semantic_id: id.to_string(),
        connector_id: Some(10),
        link: link.map(|s| s.to_string()),
        content: content.to_string(),
        chunk_count: None,
        updated_at: None,
    }
}

const GUIDE: &str = "This comprehensive operations guide describes how the homelab search \
    cluster is deployed, monitored and upgraded. It walks through Postgres tuning, \
    OpenSearch shard sizing, connector scheduling, embedding throughput, and the \
    recovery procedures used when the disk watermark trips or an index attempt stalls. \
    Each section closes with a checklist that the on-call operator is expected to follow.";

#[test]
fn yaml_config_drives_the_dedup_engine_end_to_end() {
    let yaml = r#"
dedup:
  minhash:
    num_perm: 128
    jaccard_threshold: 0.85
    shingle_size: 5
  similarity_threshold: 0.90
  report_only_low: 0.80
  prefer_keep: shortest_url
language:
  enabled: true
  allowed: [en, de]
"#;
    let config = PruneConfig::from_yaml(yaml).expect("yaml parses");
    assert_eq!(config.dedup.minhash.jaccard_threshold, 0.85);
    assert_eq!(config.dedup.prefer_keep, PreferKeepPolicy::ShortestUrl);
    assert!(config.language.enabled);

    let engine = MinHashDedupEngine::new(config.dedup.minhash.clone());

    let near_copy = format!("{GUIDE} Mirrored for the archive.");
    let docs = vec![
        make_doc("https://a/guide", Some("https://a/guide"), GUIDE),
        make_doc(
            "https://a/guide/print/view",
            Some("https://a/guide/print/view"),
            &near_copy,
        ),
        make_doc(
            "https://a/unrelated",
            Some("https://a/unrelated"),
            "A completely different page about gardening tips, soil acidity, compost \
             rotation and seasonal pruning of fruit trees in a temperate climate.",
        ),
    ];

    let pairs = engine.detect_duplicates(&docs, config.dedup.prefer_keep);
    assert_eq!(pairs.len(), 1, "only the near-copy pair is detected");
    let pair = &pairs[0];
    assert_eq!(pair.kept_document_id, "https://a/guide", "shortest URL wins");
    assert_eq!(pair.duplicate_document_id, "https://a/guide/print/view");
    assert!(pair.jaccard_similarity >= 0.85);
    assert!(pair.reason.contains("shortest URL"));
}

#[test]
fn config_export_import_round_trips_through_yaml() {
    let mut config = PruneConfig::default();
    config.language.enabled = true;
    config.language.allowed = vec!["en".into(), "fr".into()];
    config.dedup.similarity_threshold = 0.93;
    config.thin.min_age_days = 14;

    let exported = config.to_yaml().expect("exports");
    let imported = PruneConfig::from_yaml(&exported).expect("imports");
    assert_eq!(config, imported);
}

#[test]
fn signatures_survive_serialisation_boundaries() {
    // The full-corpus scan persists signatures between pages; equality after a
    // byte round-trip is what makes that checkpointing sound.
    let config = PruneConfig::default();
    let engine = MinHashDedupEngine::new(config.dedup.minhash.clone());
    let sig = engine.signature_for_text(GUIDE);

    let bytes: Vec<u8> = sig.iter().flat_map(|v| v.to_le_bytes()).collect();
    let restored: Vec<u64> = bytes
        .chunks_exact(8)
        .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
        .collect();
    assert_eq!(sig, restored);
    assert_eq!(engine.jaccard_similarity(&sig, &restored), 1.0);
}
