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
        let config = PruneConfig::from_yaml(
            "language:\n  enabled: true\n  allowed: [en, de]\n",
        )
        .unwrap();
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
        assert_eq!(
            base.with_overrides(&serde_json::Value::Null).unwrap(),
            base
        );
    }
}
