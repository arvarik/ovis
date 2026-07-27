use serde::{Deserialize, Serialize};
use std::path::Path;

/// Root configuration struct for OVIS Pruning & Deduplication Engine (`prune_config.yaml`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PruneConfig {
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub repository_scope: Option<String>,
    #[serde(default)]
    pub heuristics: HeuristicsConfig,
    #[serde(default)]
    pub deduplication: DeduplicationConfig,
    #[serde(default)]
    pub llm_relevance: LlmRelevanceConfig,
    #[serde(default)]
    pub execution: ExecutionConfig,
}

fn default_version() -> String {
    "1.0".to_string()
}

/// Configuration options for microsecond fast heuristic quality pre-filtering.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HeuristicsConfig {
    #[serde(default = "default_min_char_count")]
    pub min_char_count: usize,
    #[serde(default = "default_max_char_count")]
    pub max_char_count: usize,
    #[serde(default = "default_min_alphanumeric_ratio")]
    pub min_alphanumeric_ratio: f32,
    #[serde(default = "default_title_blacklist_regex")]
    pub title_blacklist_regex: Vec<String>,
    #[serde(default = "default_url_blacklist_patterns")]
    pub url_blacklist_patterns: Vec<String>,
}

fn default_min_char_count() -> usize {
    150
}
fn default_max_char_count() -> usize {
    300_000
}
fn default_min_alphanumeric_ratio() -> f32 {
    0.50
}
fn default_title_blacklist_regex() -> Vec<String> {
    vec![
        "(?i)^404 Not Found".to_string(),
        "(?i)^Access Denied".to_string(),
        "(?i)^Login \\| ".to_string(),
        "(?i)(404 not found|access denied|login required|enable javascript|cookie policy)"
            .to_string(),
    ]
}
fn default_url_blacklist_patterns() -> Vec<String> {
    vec![
        "*/tag/*".to_string(),
        "*/category/*".to_string(),
        "*?share=*".to_string(),
    ]
}

impl Default for HeuristicsConfig {
    fn default() -> Self {
        Self {
            min_char_count: default_min_char_count(),
            max_char_count: default_max_char_count(),
            min_alphanumeric_ratio: default_min_alphanumeric_ratio(),
            title_blacklist_regex: default_title_blacklist_regex(),
            url_blacklist_patterns: default_url_blacklist_patterns(),
        }
    }
}

/// Strategy for selecting which document to retain when a near-duplicate pair is detected.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Copy)]
#[serde(rename_all = "snake_case")]
pub enum PreferKeepPolicy {
    LongestContent,
    NewestUpdated,
    ShortestUrl,
}

impl Default for PreferKeepPolicy {
    fn default() -> Self {
        PreferKeepPolicy::LongestContent
    }
}

/// Configuration options for MinHash Locality Sensitive Hashing (LSH) near-deduplication.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeduplicationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub minhash: MinHashConfig,
    #[serde(default)]
    pub prefer_keep: PreferKeepPolicy,
}

fn default_true() -> bool {
    true
}

impl Default for DeduplicationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            minhash: MinHashConfig::default(),
            prefer_keep: PreferKeepPolicy::LongestContent,
        }
    }
}

/// MinHash LSH mathematical hyperparameters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MinHashConfig {
    #[serde(default = "default_num_perm")]
    pub num_perm: usize,
    #[serde(default = "default_jaccard_threshold")]
    pub jaccard_threshold: f64,
    #[serde(default = "default_shingle_size")]
    pub shingle_size: usize,
    #[serde(default)]
    pub bands: Option<usize>,
}

fn default_num_perm() -> usize {
    128
}
fn default_jaccard_threshold() -> f64 {
    0.85
}
fn default_shingle_size() -> usize {
    5
}

impl Default for MinHashConfig {
    fn default() -> Self {
        Self {
            num_perm: default_num_perm(),
            jaccard_threshold: default_jaccard_threshold(),
            shingle_size: default_shingle_size(),
            bands: None,
        }
    }
}

/// Optional LLM semantic relevance scoring configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LlmRelevanceConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_llm_provider")]
    pub provider: Option<String>,
    #[serde(default = "default_llm_model")]
    pub model: Option<String>,
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: Option<f32>,
}

fn default_llm_provider() -> Option<String> {
    Some("ollama".to_string())
}
fn default_llm_model() -> Option<String> {
    Some("qwen2.5-coder:7b".to_string())
}
fn default_confidence_threshold() -> Option<f32> {
    Some(0.70)
}

impl Default for LlmRelevanceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_llm_provider(),
            model: default_llm_model(),
            confidence_threshold: default_confidence_threshold(),
        }
    }
}

/// Execution mode configuration (dry-run, audit logging path).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecutionConfig {
    #[serde(default = "default_true")]
    pub dry_run: bool,
    #[serde(default)]
    pub auto_delete: bool,
    #[serde(default = "default_audit_log_path")]
    pub audit_log_path: String,
}

fn default_audit_log_path() -> String {
    "./prune_audit_log.json".to_string()
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            dry_run: true,
            auto_delete: false,
            audit_log_path: default_audit_log_path(),
        }
    }
}

impl Default for PruneConfig {
    fn default() -> Self {
        Self {
            version: default_version(),
            repository_scope: Some("Engineering documentation and technical API specs".to_string()),
            heuristics: HeuristicsConfig::default(),
            deduplication: DeduplicationConfig::default(),
            llm_relevance: LlmRelevanceConfig::default(),
            execution: ExecutionConfig::default(),
        }
    }
}

impl PruneConfig {
    /// Parse `PruneConfig` from a YAML format string.
    pub fn from_yaml(yaml_str: &str) -> anyhow::Result<Self> {
        let config: Self = serde_yaml::from_str(yaml_str)?;
        Ok(config)
    }

    /// Load and parse `PruneConfig` from a YAML file path.
    pub fn from_file<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::from_yaml(&content)
    }

    /// Serialize `PruneConfig` to a formatted YAML string.
    pub fn to_yaml(&self) -> anyhow::Result<String> {
        let s = serde_yaml::to_string(self)?;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_values() {
        let config = PruneConfig::default();
        assert_eq!(config.version, "1.0");
        assert_eq!(config.heuristics.min_char_count, 150);
        assert_eq!(config.heuristics.max_char_count, 300_000);
        assert_eq!(config.heuristics.min_alphanumeric_ratio, 0.50);
        assert_eq!(config.deduplication.minhash.num_perm, 128);
        assert_eq!(config.deduplication.minhash.jaccard_threshold, 0.85);
        assert_eq!(config.deduplication.minhash.shingle_size, 5);
        assert_eq!(
            config.deduplication.prefer_keep,
            PreferKeepPolicy::LongestContent
        );
        assert!(config.execution.dry_run);
        assert!(!config.execution.auto_delete);
    }

    #[test]
    fn test_parse_yaml_spec_example() {
        let yaml_str = r#"
version: "1.0"
repository_scope: "Engineering documentation and technical API specs for internal microservices"

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

llm_relevance:
  enabled: false
  provider: "ollama"
  model: "qwen2.5-coder:7b"
  confidence_threshold: 0.70

execution:
  dry_run: true
  auto_delete: false
  audit_log_path: "./prune_audit_log.json"
"#;

        let config = PruneConfig::from_yaml(yaml_str).expect("Failed to parse spec YAML");
        assert_eq!(config.version, "1.0");
        assert_eq!(
            config.repository_scope.as_deref(),
            Some("Engineering documentation and technical API specs for internal microservices")
        );
        assert_eq!(config.heuristics.min_char_count, 150);
        assert_eq!(config.heuristics.max_char_count, 300000);
        assert_eq!(config.heuristics.min_alphanumeric_ratio, 0.50);
        assert_eq!(config.heuristics.title_blacklist_regex.len(), 3);
        assert_eq!(config.heuristics.url_blacklist_patterns.len(), 3);
        assert_eq!(config.deduplication.minhash.num_perm, 128);
        assert_eq!(config.deduplication.minhash.jaccard_threshold, 0.85);
        assert_eq!(
            config.deduplication.prefer_keep,
            PreferKeepPolicy::LongestContent
        );
        assert!(!config.llm_relevance.enabled);
        assert!(config.execution.dry_run);
        assert_eq!(config.execution.audit_log_path, "./prune_audit_log.json");
    }

    #[test]
    fn test_yaml_roundtrip() {
        let config = PruneConfig::default();
        let yaml_str = config.to_yaml().expect("Failed to serialize to YAML");
        let parsed = PruneConfig::from_yaml(&yaml_str).expect("Failed to deserialize from YAML");
        assert_eq!(config, parsed);
    }
}
