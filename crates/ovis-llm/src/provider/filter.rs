//! Model catalog guardrails and latest-per-family filtering.
//!
//! Filters raw provider model listings down to interactive chat/reasoning
//! models and selects the latest generation per model family, preventing
//! obsolete or deprecated models from being used or probed.

use std::collections::{HashMap, HashSet};

use super::{ModelInfo, ProviderKind};

/// Substrings that identify a model as non-conversational (audio, image,
/// robotics, moderation, or specialized non-chat lines).
const NON_CHAT_PATTERNS: &[&str] = &[
    // Audio / speech / transcription
    "whisper",
    "tts",
    "-tts",
    "transcribe",
    "-audio",
    "realtime",
    "speech",
    // Image / video generation
    "dall-e",
    "dalle",
    "imagen",
    "veo",
    "-image",
    "image-generation",
    "banana",
    // Moderation & safety classifiers
    "moderation",
    "guard",
    // Specialized non-chat agents / automation
    "lyria",
    "robotics",
    "computer-use",
    "deep-research",
    "antigravity",
    // Legacy completion engines
    "babbage",
    "davinci",
    "-instruct",
    // Search / retrieval helper models
    "search-preview",
    // Attributed Q&A (not chat)
    "aqa",
];

/// Returns true if the model id appears to be a chat/reasoning model.
pub fn is_chat_model(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    !NON_CHAT_PATTERNS
        .iter()
        .any(|pattern| lower.contains(pattern))
}

/// Strip a pinned snapshot suffix from a model ID.
/// Matches: -YYYY-MM-DD, -YYYYMMDD, -MMDD, or @NNN (e.g. -2024-08-06, -20250219, -0125, @001).
pub fn base_alias_of(id: &str) -> &str {
    // If ends with @\d+
    if let Some(idx) = id.rfind('@') {
        let suffix = &id[idx + 1..];
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            return &id[..idx];
        }
    }
    // If ends with -\d{4}-\d{2}-\d{2}
    if id.len() >= 11 {
        let bytes = id.as_bytes();
        let n = id.len();
        if bytes[n - 11] == b'-'
            && bytes[n - 6] == b'-'
            && bytes[n - 3] == b'-'
            && bytes[n - 10..n - 6].iter().all(|b| b.is_ascii_digit())
            && bytes[n - 5..n - 3].iter().all(|b| b.is_ascii_digit())
            && bytes[n - 2..].iter().all(|b| b.is_ascii_digit())
        {
            return &id[..n - 11];
        }
    }
    // If ends with -\d{8} or -\d{3,4}
    if let Some(idx) = id.rfind('-') {
        let suffix = &id[idx + 1..];
        if (suffix.len() == 8 || suffix.len() == 4 || suffix.len() == 3)
            && suffix.chars().all(|c| c.is_ascii_digit())
        {
            return &id[..idx];
        }
    }
    id
}

/// Collapse alias duplicates: when both floating alias (e.g. `gpt-4o`)
/// and its pinned snapshot (`gpt-4o-2024-08-06`) appear, drop the snapshot.
pub fn dedupe_aliases(models: Vec<ModelInfo>) -> Vec<ModelInfo> {
    let ids: HashSet<String> = models.iter().map(|m| m.id.clone()).collect();
    let mut seen = HashSet::new();
    let mut result = Vec::new();

    for m in models {
        if !seen.insert(m.id.clone()) {
            continue;
        }
        let base = base_alias_of(&m.id);
        if base != m.id && ids.contains(base) {
            continue;
        }
        result.push(m);
    }
    result
}

/// Infer the model family (e.g. "Pro", "Flash", "Sonnet").
pub fn infer_model_family(kind: ProviderKind, id: &str) -> Option<&'static str> {
    let lower = id.to_ascii_lowercase();
    match kind {
        ProviderKind::Gemini => {
            if lower.starts_with("gemma") {
                Some("Gemma")
            } else if lower.starts_with("gemini") {
                if lower.contains("flash-lite") {
                    Some("Flash-Lite")
                } else if lower.contains("flash") {
                    Some("Flash")
                } else if lower.contains("pro") {
                    Some("Pro")
                } else {
                    None
                }
            } else {
                None
            }
        }
        ProviderKind::Anthropic => {
            if lower.contains("fable") {
                Some("Fable")
            } else if lower.contains("opus") {
                Some("Opus")
            } else if lower.contains("sonnet") {
                Some("Sonnet")
            } else if lower.contains("haiku") {
                Some("Haiku")
            } else {
                None
            }
        }
        ProviderKind::OpenAiCompatible => {
            if lower.contains("-sol") {
                Some("Flagship")
            } else if lower.contains("-terra") {
                Some("Balanced")
            } else if lower.contains("-luna") {
                Some("Fast")
            } else if lower.contains("nano") {
                Some("Nano")
            } else if lower.contains("mini") {
                Some("Mini")
            } else if lower.starts_with('o')
                && lower.chars().nth(1).is_some_and(|c| c.is_ascii_digit())
            {
                Some("Reasoning")
            } else if lower.contains("codex") {
                Some("Codex")
            } else if lower.starts_with("chatgpt") {
                Some("Chat")
            } else if lower.starts_with("gpt") {
                Some("Flagship")
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Extract a comparable generation number from a model ID.
/// e.g. "gemini-3.8-flash" -> 3.8, "claude-sonnet-4-6" -> 4.6, "gpt-5.6-sol" -> 5.6, "o4-mini" -> 4.0
pub fn extract_generation(id: &str) -> Option<f64> {
    let base = base_alias_of(id);
    let mut normalized = String::with_capacity(base.len());
    let chars: Vec<char> = base.chars().collect();
    for i in 0..chars.len() {
        if chars[i] == '-'
            && i > 0
            && i + 1 < chars.len()
            && chars[i - 1].is_ascii_digit()
            && chars[i + 1].is_ascii_digit()
        {
            normalized.push('.');
        } else {
            normalized.push(chars[i]);
        }
    }

    let mut num_str = String::new();
    let mut in_num = false;
    let mut has_dot = false;

    for c in normalized.chars() {
        if c.is_ascii_digit() {
            in_num = true;
            num_str.push(c);
        } else if in_num && c == '.' && !has_dot {
            has_dot = true;
            num_str.push(c);
        } else if in_num {
            break;
        }
    }

    if num_str.is_empty() {
        None
    } else {
        num_str.parse::<f64>().ok()
    }
}

/// Check if a model is an experimental / preview / floating-latest alias.
pub fn is_preview_variant(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    lower.contains("preview")
        || lower.contains("exp")
        || lower.ends_with("-latest")
        || lower.ends_with("latest")
}

const FAMILY_ORDER_GEMINI: &[&str] = &["Pro", "Flash", "Flash-Lite", "Gemma"];
const FAMILY_ORDER_ANTHROPIC: &[&str] = &["Fable", "Opus", "Sonnet", "Haiku"];
const FAMILY_ORDER_OPENAI: &[&str] = &[
    "Flagship",
    "Balanced",
    "Fast",
    "Mini",
    "Nano",
    "Reasoning",
    "Codex",
    "Chat",
];

/// Reduces a model list to the latest generation of each family.
pub fn keep_latest_per_family(kind: ProviderKind, models: Vec<ModelInfo>) -> Vec<ModelInfo> {
    let order = match kind {
        ProviderKind::Gemini => FAMILY_ORDER_GEMINI,
        ProviderKind::Anthropic => FAMILY_ORDER_ANTHROPIC,
        ProviderKind::OpenAiCompatible => FAMILY_ORDER_OPENAI,
        _ => return models,
    };

    let mut groups: HashMap<Option<&'static str>, Vec<ModelInfo>> = HashMap::new();
    for m in models {
        let family = infer_model_family(kind, &m.id);
        groups.entry(family).or_default().push(m);
    }

    let mut result = Vec::new();

    // First process recognized families in priority order
    for &fam in order {
        if let Some(group) = groups.remove(&Some(fam)) {
            let max_gen = group
                .iter()
                .filter_map(|m| extract_generation(&m.id))
                .fold(None, |acc: Option<f64>, g| {
                    Some(acc.map_or(g, |m| m.max(g)))
                });

            let mut kept = if let Some(max) = max_gen {
                let same_gen: Vec<ModelInfo> = group
                    .into_iter()
                    .filter(|m| {
                        extract_generation(&m.id).is_some_and(|g| (g - max).abs() < 0.001)
                    })
                    .collect();
                let stable: Vec<ModelInfo> = same_gen
                    .iter()
                    .filter(|m| !is_preview_variant(&m.id))
                    .cloned()
                    .collect();
                if !stable.is_empty() {
                    stable
                } else {
                    same_gen
                }
            } else {
                group
            };

            // Collapse sub-variant suffixes: if model A's id starts with (model B's id + "-"), drop model A
            let ids: Vec<String> = kept.iter().map(|m| m.id.clone()).collect();
            kept.retain(|m| {
                !ids.iter()
                    .any(|other| other != &m.id && m.id.starts_with(&format!("{other}-")))
            });

            result.extend(kept);
        }
    }

    // Then process unclassified models (and embedding models)
    if let Some(other) = groups.remove(&None) {
        result.extend(other);
    }

    result
}

/// Apply all guardrails and latest-per-family reduction.
pub fn filter_models(kind: ProviderKind, models: Vec<ModelInfo>) -> Vec<ModelInfo> {
    // 1. Modality filter: drop non-chat models (embedding models with is_embedding=true are preserved)
    let filtered: Vec<ModelInfo> = models
        .into_iter()
        .filter(|m| m.advertised.is_embedding || is_chat_model(&m.id))
        .collect();

    // 2. Alias deduplication
    let deduped = dedupe_aliases(filtered);

    // 3. Keep latest generation per family for hosted providers
    keep_latest_per_family(kind, deduped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::AdvertisedMetadata;

    fn model(id: &str) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            display_name: Some(id.to_string()),
            advertised: AdvertisedMetadata::default(),
        }
    }

    #[test]
    fn modality_filter_drops_non_chat_models() {
        assert!(!is_chat_model("gemini-2.5-flash-preview-tts"));
        assert!(!is_chat_model("lyria-3-clip-preview"));
        assert!(!is_chat_model("gemini-robotics-er-2-preview"));
        assert!(!is_chat_model("gemini-3.1-flash-image-preview"));
        assert!(!is_chat_model("whisper-1"));
        assert!(!is_chat_model("dall-e-3"));
        assert!(is_chat_model("gemini-3.8-flash"));
        assert!(is_chat_model("gemini-3.5-flash-lite"));
        assert!(is_chat_model("claude-sonnet-4-6"));
        assert!(is_chat_model("gpt-5.6-sol"));
    }

    #[test]
    fn base_alias_strips_snapshots() {
        assert_eq!(base_alias_of("gpt-4o-2024-08-06"), "gpt-4o");
        assert_eq!(base_alias_of("gemini-2.5-flash@001"), "gemini-2.5-flash");
        assert_eq!(
            base_alias_of("claude-3-5-sonnet-20241022"),
            "claude-3-5-sonnet"
        );
        assert_eq!(base_alias_of("gemini-3.8-flash"), "gemini-3.8-flash");
    }

    #[test]
    fn generation_extraction_works() {
        assert_eq!(extract_generation("gemini-3.8-flash"), Some(3.8));
        assert_eq!(extract_generation("gemini-3.5-flash-lite"), Some(3.5));
        assert_eq!(extract_generation("claude-sonnet-4-6"), Some(4.6));
        assert_eq!(extract_generation("gpt-5.6-sol"), Some(5.6));
        assert_eq!(extract_generation("o4-mini"), Some(4.0));
    }

    #[test]
    fn latest_per_family_keeps_newest_generation() {
        let models = vec![
            model("gemini-2.5-flash"),
            model("gemini-3.5-flash"),
            model("gemini-3.8-flash"),
            model("gemini-2.5-flash-lite"),
            model("gemini-3.5-flash-lite"),
            model("gemini-3.1-pro-preview"),
            model("gemini-3.1-pro-preview-customtools"),
            model("gemma-4-31b-it"),
        ];

        let filtered = filter_models(ProviderKind::Gemini, models);
        let ids: Vec<&str> = filtered.iter().map(|m| m.id.as_str()).collect();

        // Keeps latest generation of Flash (3.8), Flash-Lite (3.5), Pro (3.1), Gemma (4)
        assert_eq!(
            ids,
            vec![
                "gemini-3.1-pro-preview",
                "gemini-3.8-flash",
                "gemini-3.5-flash-lite",
                "gemma-4-31b-it"
            ]
        );
    }
}
