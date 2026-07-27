use crate::config::HeuristicsConfig;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Document content combined with metadata required for evaluation by the pruning pipeline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocumentWithContent {
    pub id: String,
    pub semantic_id: String,
    pub connector_id: i32,
    pub link: Option<String>,
    pub content: String,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    pub metadata: serde_json::Value,
}

/// Detailed reason why a page was flagged by a pruning rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PruneFlagReason {
    pub rule_name: String,
    pub description: String,
    pub confidence: f32,
}

/// Document candidate flagged for potential deletion or archival by the pruning engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PruneCandidate {
    pub document_id: String,
    pub title: String,
    pub connector_id: i32,
    pub flag_reasons: Vec<PruneFlagReason>,
    pub duplicate_of: Option<String>,
}

/// Fast microsecond heuristic evaluator for document quality and metadata filtering.
pub struct HeuristicEvaluator {
    config: HeuristicsConfig,
    title_regexes: Vec<(String, Regex)>,
    url_regexes: Vec<(String, Regex)>,
}

impl HeuristicEvaluator {
    /// Create a new `HeuristicEvaluator` from a `HeuristicsConfig`.
    pub fn new(config: HeuristicsConfig) -> anyhow::Result<Self> {
        let mut title_regexes = Vec::new();
        for pat in &config.title_blacklist_regex {
            let re = Regex::new(pat)?;
            title_regexes.push((pat.clone(), re));
        }

        let mut url_regexes = Vec::new();
        for pat in &config.url_blacklist_patterns {
            let re = glob_or_regex_to_regex(pat)?;
            url_regexes.push((pat.clone(), re));
        }

        Ok(Self {
            config,
            title_regexes,
            url_regexes,
        })
    }

    /// Access inner configuration.
    pub fn config(&self) -> &HeuristicsConfig {
        &self.config
    }

    /// Evaluate a document against all heuristic quality rules.
    pub fn evaluate(&self, doc: &DocumentWithContent) -> Vec<PruneFlagReason> {
        let mut reasons = Vec::new();

        let char_count = doc.content.chars().count();

        // 1. Min character length bound
        if char_count < self.config.min_char_count {
            reasons.push(PruneFlagReason {
                rule_name: "min_char_count".to_string(),
                description: format!(
                    "Content character count {} is below min threshold {}",
                    char_count, self.config.min_char_count
                ),
                confidence: 1.0,
            });
        }

        // 2. Max character length bound
        if char_count > self.config.max_char_count {
            reasons.push(PruneFlagReason {
                rule_name: "max_char_count".to_string(),
                description: format!(
                    "Content character count {} exceeds max threshold {}",
                    char_count, self.config.max_char_count
                ),
                confidence: 1.0,
            });
        }

        // 3. Minimum alphanumeric ratio check
        if char_count > 0 {
            let alpha_count = doc.content.chars().filter(|c| c.is_alphanumeric()).count();
            let ratio = alpha_count as f32 / char_count as f32;
            if ratio < self.config.min_alphanumeric_ratio {
                reasons.push(PruneFlagReason {
                    rule_name: "min_alphanumeric_ratio".to_string(),
                    description: format!(
                        "Alphanumeric ratio {:.4} is below min threshold {:.4}",
                        ratio, self.config.min_alphanumeric_ratio
                    ),
                    confidence: 1.0,
                });
            }
        } else if self.config.min_alphanumeric_ratio > 0.0 {
            reasons.push(PruneFlagReason {
                rule_name: "min_alphanumeric_ratio".to_string(),
                description: format!(
                    "Alphanumeric ratio 0.0000 is below min threshold {:.4}",
                    self.config.min_alphanumeric_ratio
                ),
                confidence: 1.0,
            });
        }

        // 4. Title / Boilerplate Error Page Regex
        for (pattern_str, re) in &self.title_regexes {
            if re.is_match(&doc.semantic_id) || re.is_match(&doc.content) {
                reasons.push(PruneFlagReason {
                    rule_name: "title_blacklist_regex".to_string(),
                    description: format!(
                        "Matched title/error boilerplate pattern: '{}'",
                        pattern_str
                    ),
                    confidence: 1.0,
                });
                break;
            }
        }

        // 5. URL Blacklist Patterns
        let url_to_check = doc.link.as_deref().unwrap_or(&doc.id);
        for (pattern_str, re) in &self.url_regexes {
            if re.is_match(url_to_check) {
                reasons.push(PruneFlagReason {
                    rule_name: "url_blacklist_patterns".to_string(),
                    description: format!(
                        "URL '{}' matched blacklist pattern: '{}'",
                        url_to_check, pattern_str
                    ),
                    confidence: 1.0,
                });
                break;
            }
        }

        reasons
    }
}

/// Convert a glob pattern (e.g. `*/tag/*`, `*?share=*`) or existing regex string to a compiled `Regex`.
fn glob_or_regex_to_regex(pat: &str) -> anyhow::Result<Regex> {
    // If it is already a valid regex with special anchors or case flags like (?i)
    if pat.starts_with("(?i)") || pat.starts_with('^') {
        if let Ok(re) = Regex::new(pat) {
            return Ok(re);
        }
    }

    let mut regex_str = String::from("(?i)");
    for c in pat.chars() {
        match c {
            '*' => regex_str.push_str(".*"),
            '?' => regex_str.push('.'),
            '.' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\' => {
                regex_str.push('\\');
                regex_str.push(c);
            }
            _ => regex_str.push(c),
        }
    }
    Regex::new(&regex_str).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_doc(id: &str, title: &str, link: Option<&str>, content: &str) -> DocumentWithContent {
        DocumentWithContent {
            id: id.to_string(),
            semantic_id: title.to_string(),
            connector_id: 1,
            link: link.map(|s| s.to_string()),
            content: content.to_string(),
            updated_at: None,
            metadata: json!({}),
        }
    }

    #[test]
    fn test_heuristics_min_char_count() {
        let evaluator = HeuristicEvaluator::new(HeuristicsConfig::default()).unwrap();
        let short_doc = make_doc("doc1", "Short Page", None, "Too short text");
        let reasons = evaluator.evaluate(&short_doc);

        assert!(reasons.iter().any(|r| r.rule_name == "min_char_count"));
    }

    #[test]
    fn test_heuristics_max_char_count() {
        let mut config = HeuristicsConfig::default();
        config.max_char_count = 50;

        let evaluator = HeuristicEvaluator::new(config).unwrap();
        let long_text = "a".repeat(100);
        let long_doc = make_doc("doc1", "Long Page", None, &long_text);
        let reasons = evaluator.evaluate(&long_doc);

        assert!(reasons.iter().any(|r| r.rule_name == "max_char_count"));
    }

    #[test]
    fn test_heuristics_alphanumeric_ratio() {
        let evaluator = HeuristicEvaluator::new(HeuristicsConfig::default()).unwrap();
        // Heavy symbol spam with < 0.50 alpha ratio
        let symbol_spam = "!@#$%^&*()_+$%^&*()_#$%^&*()_!@#$%^&*()_+$%^&*()_#$%^&*()_!@#$%^&*()_+$%^&*()_#$%^&*()_!@#$%^&*()_+$%^&*()_#$%^&*()_!@#$%^&*()_+$%^&*()_#$%^&*()_ hello world";
        let doc = make_doc("doc1", "Symbol Spam", None, symbol_spam);
        let reasons = evaluator.evaluate(&doc);

        assert!(reasons
            .iter()
            .any(|r| r.rule_name == "min_alphanumeric_ratio"));
    }

    #[test]
    fn test_heuristics_title_blacklist_regex() {
        let evaluator = HeuristicEvaluator::new(HeuristicsConfig::default()).unwrap();
        let error_doc = make_doc(
            "doc1",
            "404 Not Found",
            None,
            "Sorry, the requested document or resource was not found on this server. Please check your URL.",
        );
        let reasons = evaluator.evaluate(&error_doc);

        assert!(reasons
            .iter()
            .any(|r| r.rule_name == "title_blacklist_regex"));
    }

    #[test]
    fn test_heuristics_url_blacklist_pattern() {
        let evaluator = HeuristicEvaluator::new(HeuristicsConfig::default()).unwrap();
        let tag_doc = make_doc(
            "doc1",
            "Tag Archive",
            Some("https://example.com/blog/tag/rust-programming"),
            "This is a tag page collecting posts about Rust programming with plenty of characters to pass min length threshold.",
        );
        let reasons = evaluator.evaluate(&tag_doc);

        assert!(reasons
            .iter()
            .any(|r| r.rule_name == "url_blacklist_patterns"));
    }

    #[test]
    fn test_heuristics_valid_doc_passes() {
        let evaluator = HeuristicEvaluator::new(HeuristicsConfig::default()).unwrap();
        let valid_content = "This is a legitimate technical document describing the architecture and operation of microservices in our system. It contains structured text, clean sentences, normal punctuation, and clear code explanations without filler.";
        let valid_doc = make_doc(
            "doc1",
            "Architecture Overview",
            Some("https://docs.company.internal/arch/overview"),
            valid_content,
        );
        let reasons = evaluator.evaluate(&valid_doc);

        assert!(
            reasons.is_empty(),
            "Valid document should pass all heuristic checks"
        );
    }
}
