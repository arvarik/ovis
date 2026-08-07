//! Detector configuration, with the YAML round-trip that is the config-file
//! story (`ovis prune config export|import`).
//!
//! Every default is conservative (N5): content detectors ship disabled or
//! report-only, thresholds match `redesign/prune/01_DETECTION_STRATEGY.md`,
//! and partial YAML works — every field has a serde default.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Root configuration for the pruning detectors.
///
/// This is the *detector* config — lifecycle settings (grace period, reaper
/// rates, batch guards) are server configuration, not scan configuration, and
/// live in the backend's environment.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PruneConfig {
    pub version: String,
    pub dedup: DedupConfig,
    pub language: LanguageConfig,
    pub thin: ThinConfig,
    pub stale: StaleConfig,
    pub llm_relevance: LlmRelevanceConfig,
    pub quality: QualityConfig,
    pub url_junk: UrlJunkConfig,
    pub semantic: SemanticConfig,
}

impl PruneConfig {
    pub fn from_yaml(yaml_str: &str) -> anyhow::Result<Self> {
        Ok(serde_yaml::from_str(yaml_str)?)
    }

    pub fn from_file<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        Self::from_yaml(&std::fs::read_to_string(path)?)
    }

    pub fn to_yaml(&self) -> anyhow::Result<String> {
        Ok(serde_yaml::to_string(self)?)
    }

    /// Apply a partial JSON override (per-scan `config_overrides`): objects
    /// merge recursively, everything else replaces. Unknown keys are an error —
    /// a typo'd override must not silently change nothing.
    pub fn with_overrides(&self, overrides: &serde_json::Value) -> anyhow::Result<Self> {
        if overrides.is_null() {
            return Ok(self.clone());
        }
        let mut base = serde_json::to_value(self)?;
        merge_json(&mut base, overrides)?;
        let merged: Self = serde_json::from_value(base)
            .map_err(|e| anyhow::anyhow!("config override does not fit the schema: {e}"))?;
        Ok(merged)
    }
}

fn merge_json(base: &mut serde_json::Value, overlay: &serde_json::Value) -> anyhow::Result<()> {
    match (base, overlay) {
        (serde_json::Value::Object(base_map), serde_json::Value::Object(overlay_map)) => {
            for (key, value) in overlay_map {
                match base_map.get_mut(key) {
                    Some(slot) => merge_json(slot, value)?,
                    None => anyhow::bail!("unknown config key '{key}'"),
                }
            }
            Ok(())
        }
        (slot, value) => {
            *slot = value.clone();
            Ok(())
        }
    }
}

/// Which document of a duplicate pair/group survives.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PreferKeepPolicy {
    /// Canonical URLs are usually the short ones.
    #[default]
    ShortestUrl,
    LongestContent,
    NewestUpdated,
    MostChunks,
}

/// Near-duplicate detection (MinHash + LSH).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DedupConfig {
    pub minhash: MinHashConfig,
    /// Pairs at/above this estimated Jaccard are candidates at that
    /// confidence.
    pub similarity_threshold: f64,
    /// Pairs in `[report_only_low, similarity_threshold)` are surfaced as
    /// low-confidence, report-only reasons.
    pub report_only_low: f64,
    pub prefer_keep: PreferKeepPolicy,
}

impl Default for DedupConfig {
    fn default() -> Self {
        Self {
            minhash: MinHashConfig::default(),
            similarity_threshold: 0.90,
            report_only_low: 0.80,
            prefer_keep: PreferKeepPolicy::default(),
        }
    }
}

/// MinHash hyperparameters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct MinHashConfig {
    pub num_perm: usize,
    /// The engine-level acting threshold; pairs below it are not emitted.
    pub jaccard_threshold: f64,
    pub shingle_size: usize,
    pub bands: Option<usize>,
}

impl Default for MinHashConfig {
    fn default() -> Self {
        Self {
            num_perm: 128,
            jaccard_threshold: 0.90,
            shingle_size: 5,
            bands: None,
        }
    }
}

/// Foreign-language detection. Ships OFF: multilingual corpora are
/// legitimate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LanguageConfig {
    pub enabled: bool,
    /// ISO 639-1/639-3 codes the corpus wants to keep.
    pub allowed: Vec<String>,
    /// Below this detector confidence the document is NOT flagged.
    pub min_confidence: f64,
    /// Shorter texts misdetect; skip them.
    pub min_text_len: usize,
    /// Keyed by connector name, e.g. a linguistics site that legitimately
    /// hosts many languages.
    pub per_connector_overrides: BTreeMap<String, LanguageOverride>,
}

impl Default for LanguageConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed: vec!["en".to_string()],
            min_confidence: 0.85,
            min_text_len: 200,
            per_connector_overrides: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LanguageOverride {
    pub enabled: bool,
}

/// Thin content: stubs (0 chunks) and near-empty pages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ThinConfig {
    /// A 0-chunk document younger than this is never flagged — it may simply
    /// not be chunked yet. (`chunk_count: null` is never flagged at any age.)
    pub min_age_days: i64,
    /// Fetched text below this word count flags at `short_confidence`.
    pub min_words: usize,
    /// Confidence for the 0-chunk stub reason.
    pub stub_confidence: f32,
    /// Confidence for the below-`min_words` reason.
    pub short_confidence: f32,
}

impl Default for ThinConfig {
    fn default() -> Self {
        Self {
            min_age_days: 7,
            min_words: 40,
            stub_confidence: 0.9,
            short_confidence: 0.6,
        }
    }
}

/// Staleness is corpus policy, not junk — report-only and OFF by default.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct StaleConfig {
    pub enabled: bool,
    pub older_than_days: i64,
    pub confidence: f32,
}

impl Default for StaleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            older_than_days: 730,
            confidence: 0.5,
        }
    }
}

/// Model-free text quality gates.
///
/// Every threshold is the published value from the pipeline named beside it —
/// Gopher (arXiv 2112.11446), FineWeb (arXiv 2406.17557), C4 (arXiv
/// 1910.10683). They are tuned on web crawls, which is what this corpus is.
///
/// The gates *measure* on every scan; `min_failures` decides how many must
/// trip before a document becomes a candidate, and that is review-time policy
/// rather than a scan-time constant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct QualityConfig {
    /// Gopher: fewer words than this is not a document.
    pub min_words: usize,
    /// Gopher: more words than this is a concatenation artefact.
    pub max_words: usize,
    pub min_mean_word_length: f32,
    pub max_mean_word_length: f32,
    /// Gopher: `#` and `…` per word.
    pub max_symbol_word_ratio: f32,
    pub max_bullet_line_fraction: f32,
    pub max_ellipsis_line_fraction: f32,
    pub min_alpha_word_fraction: f32,
    /// Gopher requires at least this many of eight common English stopwords.
    pub min_stopwords: usize,
    /// The stopword and unterminated-line gates assume English prose; both are
    /// skipped below this word count, and the stopword gate can be turned off
    /// wholesale for a multilingual corpus.
    pub stopwords_enabled: bool,
    pub stopword_min_words: usize,
    /// Gopher's repetition rules assume a document with enough text for a
    /// repeated phrase to mean something. Below this word count they are
    /// skipped: in a 60-word page one twice-repeated bigram is already a fifth
    /// of the characters, which says nothing about quality.
    pub repetition_min_words: usize,
    pub max_dup_line_fraction: f32,
    pub max_dup_line_char_fraction: f32,
    pub max_top_2gram_char_fraction: f32,
    pub max_top_3gram_char_fraction: f32,
    pub max_top_4gram_char_fraction: f32,
    pub max_dup_5gram_char_fraction: f32,
    pub max_dup_10gram_char_fraction: f32,
    /// FineWeb: pages whose lines mostly do not end in punctuation are menus.
    pub min_punct_terminated_line_fraction: f32,
    pub max_short_line_fraction: f32,
    pub max_newline_word_ratio: f32,
    /// How many gates must fail before the document is flagged.
    ///
    /// Ships at three rather than two. Measured on the reference corpus, two
    /// gates flag 27% of documents and that population includes technical
    /// reference pages (API docs, HOWTOs) whose code blocks and tables trip
    /// the line-shape gates while being exactly the content worth keeping.
    /// Three flags ~14%. The Standard preset lowers it to two with a live
    /// preview of what that adds.
    pub min_failures: usize,
    /// How many distinct gate *families* the failures must span. Line-shape
    /// gates fire together on the same underlying property, so three of them
    /// is one observation, not three.
    pub min_families: usize,
    /// Connectors whose documents are never flagged by quality gates —
    /// the escape hatch for a source that is legitimately code, tables or
    /// reference material end to end.
    pub exempt_connectors: Vec<String>,
    /// Confidence when exactly `min_failures` gates trip; each further failure
    /// adds `confidence_per_extra_failure`, capped at `max_confidence`.
    pub base_confidence: f32,
    pub confidence_per_extra_failure: f32,
    pub max_confidence: f32,
}

impl Default for QualityConfig {
    fn default() -> Self {
        Self {
            min_words: 50,
            max_words: 100_000,
            min_mean_word_length: 3.0,
            max_mean_word_length: 10.0,
            max_symbol_word_ratio: 0.1,
            max_bullet_line_fraction: 0.9,
            max_ellipsis_line_fraction: 0.3,
            min_alpha_word_fraction: 0.8,
            min_stopwords: 2,
            stopwords_enabled: true,
            stopword_min_words: 50,
            repetition_min_words: 100,
            max_dup_line_fraction: 0.3,
            max_dup_line_char_fraction: 0.2,
            max_top_2gram_char_fraction: 0.20,
            max_top_3gram_char_fraction: 0.18,
            max_top_4gram_char_fraction: 0.16,
            max_dup_5gram_char_fraction: 0.15,
            max_dup_10gram_char_fraction: 0.10,
            min_punct_terminated_line_fraction: 0.12,
            max_short_line_fraction: 0.67,
            max_newline_word_ratio: 0.3,
            min_failures: 3,
            min_families: 2,
            exempt_connectors: Vec::new(),
            base_confidence: 0.55,
            confidence_per_extra_failure: 0.1,
            max_confidence: 0.85,
        }
    }
}

/// Structural URL signals: assets indexed as pages, and archive-edition
/// mirrors of a live page.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct UrlJunkConfig {
    /// Flag image/media/archive URLs whose extracted text is a crawl artefact.
    pub flag_assets: bool,
    /// PDFs and office documents are real content on this corpus, so they are
    /// never assets unless a deployment says otherwise.
    pub flag_binary_documents: bool,
    /// An asset with more than this many chunks has real extracted text
    /// (a PDF-backed image, an OCR'd scan) and is left alone.
    pub asset_max_chunks: i32,
    pub asset_confidence: f32,
    /// Flag `/archives/<edition>/…` pages whose live counterpart is indexed.
    pub flag_archive_editions: bool,
    pub archive_edition_confidence: f32,
    /// Group documents whose canonical URL key matches, keeping one.
    pub flag_url_variants: bool,
    pub url_variant_confidence: f32,
    /// Connector names exempt from every URL rule here.
    pub exempt_connectors: Vec<String>,
}

impl Default for UrlJunkConfig {
    fn default() -> Self {
        Self {
            flag_assets: true,
            flag_binary_documents: false,
            asset_max_chunks: 1,
            asset_confidence: 0.85,
            flag_archive_editions: false,
            archive_edition_confidence: 0.8,
            flag_url_variants: true,
            url_variant_confidence: 0.9,
            exempt_connectors: Vec::new(),
        }
    }
}

/// Semantic duplicate and off-topic detection over the embedding vectors the
/// index already holds.
///
/// Ships **disabled**: cosine thresholds do not transfer between embedding
/// spaces (SemDeDup's LAION ε of 0.00095 is the canonical warning), so a
/// deployment must calibrate before acting. `ovis prune calibrate` writes the
/// measured values back here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SemanticConfig {
    pub enabled: bool,
    /// Cosine at/above which two documents are the same document.
    pub duplicate_threshold: f64,
    /// Cosine at/above which a pair is surfaced for review.
    pub review_threshold: f64,
    /// Neighbours requested per document.
    pub neighbours: i64,
    /// Restrict neighbour search to the same connector. FineWeb's central
    /// finding is that global dedup over-prunes; within-source duplicates are
    /// the safe population.
    pub within_connector_only: bool,
    /// Documents below this percentile of similarity-to-connector-centroid are
    /// surfaced as off-topic. Rank-based, never an absolute cosine.
    pub off_topic_percentile: f64,
    pub off_topic_confidence: f32,
    /// Written by the calibration pass so the UI can show what the thresholds
    /// were derived from.
    pub calibration: Option<SemanticCalibration>,
}

impl Default for SemanticConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            duplicate_threshold: 0.97,
            review_threshold: 0.93,
            neighbours: 10,
            within_connector_only: true,
            off_topic_percentile: 0.5,
            off_topic_confidence: 0.4,
            calibration: None,
        }
    }
}

/// The measured background similarity distribution a threshold was chosen
/// against.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SemanticCalibration {
    pub sampled_pairs: usize,
    pub background_mean: f64,
    pub background_stddev: f64,
    /// `background_mean + n * stddev` for the chosen `duplicate_threshold`.
    pub sigma_above_background: f64,
    pub calibrated_at: String,
}

/// LLM relevance scoring — designed for, disabled, not v1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LlmRelevanceConfig {
    pub enabled: bool,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    /// The user-written statement of what the corpus is *for*, which is what
    /// the scorer would judge relevance against.
    pub corpus_intent: Option<String>,
    pub confidence_threshold: f32,
}

impl Default for LlmRelevanceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: None,
            model: None,
            corpus_intent: None,
            confidence_threshold: 0.70,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_documented_conservative_ones() {
        let config = PruneConfig::default();
        assert_eq!(config.dedup.similarity_threshold, 0.90);
        assert_eq!(config.dedup.report_only_low, 0.80);
        assert_eq!(config.dedup.prefer_keep, PreferKeepPolicy::ShortestUrl);
        assert_eq!(config.dedup.minhash.num_perm, 128);
        assert_eq!(config.dedup.minhash.shingle_size, 5);
        assert!(!config.language.enabled, "language detection ships OFF");
        assert_eq!(config.language.allowed, vec!["en"]);
        assert_eq!(config.language.min_confidence, 0.85);
        assert_eq!(config.language.min_text_len, 200);
        assert_eq!(config.thin.min_age_days, 7);
        assert_eq!(config.thin.min_words, 40);
        assert!(!config.stale.enabled, "staleness is policy, ships OFF");
        assert!(!config.llm_relevance.enabled, "LLM scoring is not v1");
    }

    #[test]
    fn yaml_round_trip_is_lossless() {
        let config = PruneConfig::default();
        let yaml = config.to_yaml().unwrap();
        let parsed = PruneConfig::from_yaml(&yaml).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn partial_yaml_fills_the_rest_with_defaults() {
        let config =
            PruneConfig::from_yaml("language:\n  enabled: true\n  allowed: [en, de]\n").unwrap();
        assert!(config.language.enabled);
        assert_eq!(config.language.allowed, vec!["en", "de"]);
        // Untouched sections keep their defaults.
        assert_eq!(config.dedup.similarity_threshold, 0.90);
        assert_eq!(config.thin.min_age_days, 7);
    }

    #[test]
    fn per_connector_language_overrides_parse() {
        let config = PruneConfig::from_yaml(
            "language:\n  enabled: true\n  per_connector_overrides:\n    wals-online:\n      enabled: false\n",
        )
        .unwrap();
        assert!(!config.language.per_connector_overrides["wals-online"].enabled);
    }

    #[test]
    fn overrides_merge_recursively_and_replace_scalars() {
        let base = PruneConfig::default();
        let merged = base
            .with_overrides(&serde_json::json!({
                "dedup": { "similarity_threshold": 0.95 },
                "thin": { "min_age_days": 14 }
            }))
            .unwrap();
        assert_eq!(merged.dedup.similarity_threshold, 0.95);
        assert_eq!(merged.thin.min_age_days, 14);
        // Sibling keys survive the merge.
        assert_eq!(merged.dedup.report_only_low, 0.80);
        assert_eq!(merged.thin.min_words, 40);
    }

    #[test]
    fn a_typo_in_an_override_is_an_error_not_a_silent_noop() {
        let err = PruneConfig::default()
            .with_overrides(&serde_json::json!({ "dedupe": { "similarity_threshold": 0.5 } }))
            .unwrap_err();
        assert!(err.to_string().contains("dedupe"), "{err}");
    }

    #[test]
    fn null_overrides_are_the_identity() {
        let base = PruneConfig::default();
        assert_eq!(base.with_overrides(&serde_json::Value::Null).unwrap(), base);
    }
}
