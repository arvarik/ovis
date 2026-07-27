//! Content-level detectors: language identification and thin-content
//! measurement. Pure functions over text the backend already fetched from the
//! chunk index — nothing here performs I/O, and nothing ever re-fetches a
//! source URL.

use serde::{Deserialize, Serialize};

use crate::config::LanguageConfig;

/// Outcome of language detection over a document's sampled text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LanguageVerdict {
    /// ISO 639-3 code, e.g. `deu`.
    pub detected: String,
    /// Detector confidence 0.0–1.0.
    pub confidence: f64,
    /// Whether the detected language is in the allowed list.
    pub allowed: bool,
    /// Bytes of text examined.
    pub sample_len: usize,
    /// Set when per-chunk detection disagreed (mixed-language document); the
    /// caller reduces confidence and shows the split.
    pub mixed_with: Option<String>,
}

/// Detect the language of `chunks` (usually the first two chunks of a
/// document) against the config.
///
/// Returns `None` when the document should not be judged at all: detection
/// disabled, text shorter than `min_text_len`, or detector confidence below
/// `min_confidence` — an unconfident guess must not become a prune reason.
pub fn detect_language(chunks: &[&str], config: &LanguageConfig) -> Option<LanguageVerdict> {
    let joined = chunks.join("\n");
    let text = joined.trim();
    if text.len() < config.min_text_len {
        return None;
    }

    let info = whatlang::detect(text)?;
    if info.confidence() < config.min_confidence {
        return None;
    }

    let detected = info.lang().code().to_string();
    let allowed = is_allowed(&detected, &config.allowed);

    // Per-chunk cross-check: a document whose chunks confidently disagree is
    // mixed-language, which the caller flags at reduced confidence.
    let mut mixed_with = None;
    if chunks.len() >= 2 {
        let mut seen = Vec::new();
        for chunk in chunks {
            if chunk.trim().len() < config.min_text_len {
                continue;
            }
            if let Some(chunk_info) = whatlang::detect(chunk.trim()) {
                if chunk_info.confidence() >= config.min_confidence {
                    let code = chunk_info.lang().code().to_string();
                    if !seen.contains(&code) {
                        seen.push(code);
                    }
                }
            }
        }
        if seen.len() > 1 {
            mixed_with = seen.into_iter().find(|c| *c != detected);
        }
    }

    Some(LanguageVerdict {
        detected,
        confidence: info.confidence(),
        allowed,
        sample_len: text.len(),
        mixed_with,
    })
}

/// Whether a detected ISO 639-3 code matches an allow-list that may use
/// two-letter (639-1) or three-letter (639-3) codes.
pub fn is_allowed(detected_639_3: &str, allowed: &[String]) -> bool {
    allowed.iter().any(|entry| {
        let entry = entry.trim().to_lowercase();
        entry == detected_639_3 || iso639_1_to_3(&entry) == Some(detected_639_3)
    })
}

/// The 639-1 → 639-3 mapping for the languages whatlang can detect. Codes not
/// listed simply never match, which fails toward *not* flagging.
fn iso639_1_to_3(two: &str) -> Option<&'static str> {
    Some(match two {
        "en" => "eng",
        "de" => "deu",
        "fr" => "fra",
        "es" => "spa",
        "it" => "ita",
        "nl" => "nld",
        "pt" => "por",
        "ru" => "rus",
        "uk" => "ukr",
        "pl" => "pol",
        "cs" => "ces",
        "sk" => "slk",
        "sl" => "slv",
        "hr" => "hrv",
        "sr" => "srp",
        "bg" => "bul",
        "ro" => "ron",
        "hu" => "hun",
        "el" => "ell",
        "tr" => "tur",
        "sv" => "swe",
        "da" => "dan",
        "nb" | "no" => "nob",
        "nn" => "nno",
        "fi" => "fin",
        "et" => "est",
        "lv" => "lav",
        "lt" => "lit",
        "ar" => "ara",
        "he" => "heb",
        "fa" => "pes",
        "hi" => "hin",
        "bn" => "ben",
        "ur" => "urd",
        "ta" => "tam",
        "te" => "tel",
        "mr" => "mar",
        "gu" => "guj",
        "kn" => "kan",
        "ml" => "mal",
        "pa" => "pan",
        "th" => "tha",
        "vi" => "vie",
        "id" => "ind",
        "ms" => "zsm",
        "tl" => "tgl",
        "ja" => "jpn",
        "ko" => "kor",
        "zh" => "cmn",
        "ka" => "kat",
        "hy" => "hye",
        "az" => "azj",
        "kk" => "kaz",
        "uz" => "uzn",
        "am" => "amh",
        "yo" => "yor",
        "zu" => "zul",
        "af" => "afr",
        "sw" => "swa",
        "eo" => "epo",
        "la" => "lat",
        "cy" => "cym",
        "be" => "bel",
        "mk" => "mkd",
        "sq" => "sqi",
        "ca" => "cat",
        "gl" => "glg",
        "eu" => "eus",
        _ => return None,
    })
}

/// Whitespace word count — the thin-content measure. Deliberately the same
/// simple heuristic as the API's `token_estimate`: honest about being a word
/// count, not a tokeniser.
pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GERMAN: &str = "Dieses Impressum enthält die gesetzlich vorgeschriebenen Angaben über \
        den Betreiber dieser Webseite sowie Hinweise zur Haftung für Inhalte und Links. \
        Verantwortlich für den Inhalt nach § 55 Abs. 2 RStV ist die Geschäftsführung der \
        Beispiel GmbH mit Sitz in Berlin. Alle Rechte vorbehalten.";

    const ENGLISH: &str = "This operations handbook explains how the search cluster is \
        deployed, monitored and upgraded, including Postgres tuning, shard sizing, \
        connector scheduling and the recovery steps for a tripped disk watermark.";

    fn config() -> LanguageConfig {
        LanguageConfig {
            enabled: true,
            ..LanguageConfig::default()
        }
    }

    #[test]
    fn german_text_is_detected_and_disallowed_under_english_only() {
        let verdict = detect_language(&[GERMAN], &config()).expect("confident detection");
        assert_eq!(verdict.detected, "deu");
        assert!(!verdict.allowed);
        assert!(verdict.confidence >= 0.85);
        assert!(verdict.mixed_with.is_none());
    }

    #[test]
    fn english_text_is_allowed_under_the_default_list() {
        let verdict = detect_language(&[ENGLISH], &config()).expect("confident detection");
        assert_eq!(verdict.detected, "eng");
        assert!(verdict.allowed);
    }

    #[test]
    fn short_texts_are_never_judged() {
        assert!(
            detect_language(&["Impressum"], &config()).is_none(),
            "short texts misdetect; the gate must hold"
        );
    }

    #[test]
    fn allow_lists_accept_both_two_and_three_letter_codes() {
        assert!(is_allowed("deu", &["de".into()]));
        assert!(is_allowed("deu", &["deu".into()]));
        assert!(is_allowed("eng", &["EN".into()]), "case-insensitive");
        assert!(!is_allowed("deu", &["en".into()]));
        assert!(
            !is_allowed("eng", &["klingon".into()]),
            "unknown allow-list entries never match"
        );
    }

    #[test]
    fn mixed_language_documents_carry_the_split() {
        let verdict =
            detect_language(&[ENGLISH, GERMAN], &config()).expect("confident detection");
        assert!(
            verdict.mixed_with.is_some(),
            "chunk disagreement must be surfaced: {verdict:?}"
        );
    }

    #[test]
    fn word_count_is_a_plain_whitespace_count() {
        assert_eq!(word_count("one two  three\nfour"), 4);
        assert_eq!(word_count("   "), 0);
    }
}
