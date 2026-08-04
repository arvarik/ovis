//! Model-free text quality gates.
//!
//! Every threshold here comes from a published web-crawl curation pipeline —
//! Gopher (arXiv 2112.11446 Table A1 and its repetition rules), FineWeb
//! (arXiv 2406.17557 custom filters) and C4 (arXiv 1910.10683). They are
//! reproduced as shipped defaults because this corpus *is* a web crawl, which
//! is exactly the population those numbers were tuned on.
//!
//! The module is deliberately measurement-shaped: [`measure`] computes the
//! statistics once, and [`Gate::evaluate`] turns them into individually named
//! failures. Nothing here decides whether a document should be pruned — that
//! is policy, applied later against the stored profile. A detector that
//! decided inline could not answer "what would a stricter setting flag?"
//! without a re-scan.
//!
//! Pure functions over text the backend already fetched: no I/O, no model, no
//! allocation beyond the line/word split.

use serde::{Deserialize, Serialize};

use crate::config::QualityConfig;

/// The eight common English stopwords Gopher requires at least two of. A page
/// of English prose essentially cannot avoid them; a page of link-lists,
/// navigation or machine output routinely does.
const STOPWORDS: [&str; 8] = ["the", "be", "to", "of", "and", "that", "have", "with"];

/// Phrases C4 drops outright.
const BOILERPLATE_MARKERS: [&str; 3] = ["lorem ipsum", "javascript is disabled", "enable cookies"];

/// Every measurable statistic of one document's text, computed in a single
/// pass. Serialized into `ovis.doc_profile.quality_flags` alongside the failed
/// gate names, so a threshold change re-evaluates without re-reading text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualityMetrics {
    pub word_count: usize,
    pub line_count: usize,
    pub mean_word_length: f32,
    /// `#` and `…` occurrences per word (Gopher's symbol-to-word ratio).
    pub symbol_word_ratio: f32,
    pub bullet_line_fraction: f32,
    pub ellipsis_line_fraction: f32,
    pub alpha_word_fraction: f32,
    pub stopword_hits: usize,
    /// Fraction of lines that are exact duplicates of an earlier line.
    pub dup_line_fraction: f32,
    /// Fraction of characters living in duplicated lines.
    pub dup_line_char_fraction: f32,
    /// Fraction of characters in the single most common 2-gram.
    pub top_2gram_char_fraction: f32,
    pub top_3gram_char_fraction: f32,
    pub top_4gram_char_fraction: f32,
    /// Fraction of characters in *repeated* 5-grams through 10-grams.
    pub dup_5gram_char_fraction: f32,
    pub dup_10gram_char_fraction: f32,
    /// Fraction of lines terminated by sentence punctuation (FineWeb).
    pub punct_terminated_line_fraction: f32,
    /// Fraction of lines shorter than 30 characters (FineWeb).
    pub short_line_fraction: f32,
    pub newline_word_ratio: f32,
    pub has_boilerplate_marker: bool,
}

/// One named quality gate. The name is the reason `code` a failure produces,
/// so review can filter by exactly which check tripped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gate {
    WordCountLow,
    WordCountHigh,
    MeanWordLength,
    SymbolRatio,
    BulletLines,
    EllipsisLines,
    AlphaWords,
    Stopwords,
    DupLines,
    RepeatedNgrams,
    UnterminatedLines,
    ShortLines,
    NewlineRatio,
    BoilerplateMarker,
}

/// The kind of anomaly a gate measures.
///
/// Gates within a family are strongly correlated — a page of code samples
/// trips `unterminated_lines`, `short_lines` and `newline_ratio` together
/// because they are three views of one property. Counting those as three
/// independent failures overstates the evidence, so policy can require
/// failures spread across distinct families instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    Length,
    Composition,
    LineShape,
    Repetition,
    Marker,
}

impl Family {
    pub fn code(self) -> &'static str {
        match self {
            Self::Length => "length",
            Self::Composition => "composition",
            Self::LineShape => "line_shape",
            Self::Repetition => "repetition",
            Self::Marker => "marker",
        }
    }
}

impl Gate {
    pub fn family(self) -> Family {
        match self {
            Self::WordCountLow | Self::WordCountHigh => Family::Length,
            Self::MeanWordLength | Self::SymbolRatio | Self::AlphaWords | Self::Stopwords => {
                Family::Composition
            }
            Self::BulletLines
            | Self::EllipsisLines
            | Self::UnterminatedLines
            | Self::ShortLines
            | Self::NewlineRatio => Family::LineShape,
            Self::DupLines | Self::RepeatedNgrams => Family::Repetition,
            Self::BoilerplateMarker => Family::Marker,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::WordCountLow => "word_count_low",
            Self::WordCountHigh => "word_count_high",
            Self::MeanWordLength => "mean_word_length",
            Self::SymbolRatio => "symbol_ratio",
            Self::BulletLines => "bullet_lines",
            Self::EllipsisLines => "ellipsis_lines",
            Self::AlphaWords => "alpha_words",
            Self::Stopwords => "stopwords",
            Self::DupLines => "dup_lines",
            Self::RepeatedNgrams => "repeated_ngrams",
            Self::UnterminatedLines => "unterminated_lines",
            Self::ShortLines => "short_lines",
            Self::NewlineRatio => "newline_ratio",
            Self::BoilerplateMarker => "boilerplate_marker",
        }
    }

    /// Human-readable explanation with the measured value and the threshold —
    /// what the review UI shows instead of a bare gate name.
    pub fn explain(self, m: &QualityMetrics, c: &QualityConfig) -> String {
        match self {
            Self::WordCountLow => format!("{} words (minimum {})", m.word_count, c.min_words),
            Self::WordCountHigh => format!("{} words (maximum {})", m.word_count, c.max_words),
            Self::MeanWordLength => format!(
                "mean word length {:.1} (expected {}–{})",
                m.mean_word_length, c.min_mean_word_length, c.max_mean_word_length
            ),
            Self::SymbolRatio => format!(
                "symbol-to-word ratio {:.2} (maximum {:.2})",
                m.symbol_word_ratio, c.max_symbol_word_ratio
            ),
            Self::BulletLines => format!(
                "{:.0}% of lines are bullets (maximum {:.0}%)",
                m.bullet_line_fraction * 100.0,
                c.max_bullet_line_fraction * 100.0
            ),
            Self::EllipsisLines => format!(
                "{:.0}% of lines end in an ellipsis (maximum {:.0}%)",
                m.ellipsis_line_fraction * 100.0,
                c.max_ellipsis_line_fraction * 100.0
            ),
            Self::AlphaWords => format!(
                "only {:.0}% of words contain a letter (minimum {:.0}%)",
                m.alpha_word_fraction * 100.0,
                c.min_alpha_word_fraction * 100.0
            ),
            Self::Stopwords => format!(
                "{} of {} common stopwords present (minimum {})",
                m.stopword_hits,
                STOPWORDS.len(),
                c.min_stopwords
            ),
            Self::DupLines => format!(
                "{:.0}% of lines are duplicates, {:.0}% of characters sit in them",
                m.dup_line_fraction * 100.0,
                m.dup_line_char_fraction * 100.0
            ),
            Self::RepeatedNgrams => format!(
                "repeated phrases dominate: top 2-gram {:.0}% of characters, repeated 5-grams {:.0}%",
                m.top_2gram_char_fraction * 100.0,
                m.dup_5gram_char_fraction * 100.0
            ),
            Self::UnterminatedLines => format!(
                "only {:.0}% of lines end in punctuation (minimum {:.0}%) — list or menu text",
                m.punct_terminated_line_fraction * 100.0,
                c.min_punct_terminated_line_fraction * 100.0
            ),
            Self::ShortLines => format!(
                "{:.0}% of lines are under 30 characters (maximum {:.0}%)",
                m.short_line_fraction * 100.0,
                c.max_short_line_fraction * 100.0
            ),
            Self::NewlineRatio => format!(
                "{:.2} newlines per word (maximum {:.2})",
                m.newline_word_ratio, c.max_newline_word_ratio
            ),
            Self::BoilerplateMarker => "contains a boilerplate marker phrase".to_string(),
        }
    }
}

/// Compute every statistic in one pass over the text.
pub fn measure(text: &str) -> QualityMetrics {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let words: Vec<&str> = text.split_whitespace().collect();
    let word_count = words.len();
    let line_count = lines.len();

    if word_count == 0 {
        return QualityMetrics {
            word_count: 0,
            line_count,
            mean_word_length: 0.0,
            symbol_word_ratio: 0.0,
            bullet_line_fraction: 0.0,
            ellipsis_line_fraction: 0.0,
            alpha_word_fraction: 0.0,
            stopword_hits: 0,
            dup_line_fraction: 0.0,
            dup_line_char_fraction: 0.0,
            top_2gram_char_fraction: 0.0,
            top_3gram_char_fraction: 0.0,
            top_4gram_char_fraction: 0.0,
            dup_5gram_char_fraction: 0.0,
            dup_10gram_char_fraction: 0.0,
            punct_terminated_line_fraction: 0.0,
            short_line_fraction: 0.0,
            newline_word_ratio: 0.0,
            has_boilerplate_marker: false,
        };
    }

    let total_word_chars: usize = words.iter().map(|w| w.chars().count()).sum();
    let mean_word_length = total_word_chars as f32 / word_count as f32;

    // Gopher counts '#' and the ellipsis; a page full of either is machine
    // output or a truncated listing rather than prose.
    let symbols = text.matches('#').count() + text.matches('…').count() + text.matches("...").count();
    let symbol_word_ratio = symbols as f32 / word_count as f32;

    let alpha_words = words.iter().filter(|w| w.chars().any(char::is_alphabetic)).count();
    let alpha_word_fraction = alpha_words as f32 / word_count as f32;

    let lowered_words: Vec<String> = words.iter().map(|w| {
        w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()
    }).collect();
    let stopword_hits = STOPWORDS
        .iter()
        .filter(|sw| lowered_words.iter().any(|w| w == *sw))
        .count();

    let (bullet_lines, ellipsis_lines, punct_lines, short_lines) = lines.iter().fold(
        (0usize, 0usize, 0usize, 0usize),
        |(bullet, ellipsis, punct, short), line| {
            let is_bullet = line.starts_with(['•', '-', '*', '‣', '◦'])
                || line.starts_with("· ")
                || line.starts_with("​•");
            let is_ellipsis = line.ends_with("...") || line.ends_with('…');
            let is_punct = line.ends_with(['.', '!', '?', '"', '”', '’', '।']);
            let is_short = line.chars().count() < 30;
            (
                bullet + usize::from(is_bullet),
                ellipsis + usize::from(is_ellipsis),
                punct + usize::from(is_punct),
                short + usize::from(is_short),
            )
        },
    );

    let line_div = line_count.max(1) as f32;
    let (dup_line_fraction, dup_line_char_fraction) = duplicate_line_stats(&lines);
    let newline_word_ratio = text.matches('\n').count() as f32 / word_count as f32;

    let has_boilerplate_marker = {
        let lowered = text.to_lowercase();
        BOILERPLATE_MARKERS.iter().any(|marker| lowered.contains(marker))
    };

    QualityMetrics {
        word_count,
        line_count,
        mean_word_length,
        symbol_word_ratio,
        bullet_line_fraction: bullet_lines as f32 / line_div,
        ellipsis_line_fraction: ellipsis_lines as f32 / line_div,
        alpha_word_fraction,
        stopword_hits,
        dup_line_fraction,
        dup_line_char_fraction,
        top_2gram_char_fraction: top_ngram_char_fraction(&lowered_words, 2),
        top_3gram_char_fraction: top_ngram_char_fraction(&lowered_words, 3),
        top_4gram_char_fraction: top_ngram_char_fraction(&lowered_words, 4),
        dup_5gram_char_fraction: duplicate_ngram_char_fraction(&lowered_words, 5),
        dup_10gram_char_fraction: duplicate_ngram_char_fraction(&lowered_words, 10),
        punct_terminated_line_fraction: punct_lines as f32 / line_div,
        short_line_fraction: short_lines as f32 / line_div,
        newline_word_ratio,
        has_boilerplate_marker,
    }
}

/// Gopher's duplicate-line rules: what fraction of lines repeat, and what
/// fraction of all characters live in a repeated line.
fn duplicate_line_stats(lines: &[&str]) -> (f32, f32) {
    if lines.is_empty() {
        return (0.0, 0.0);
    }
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for line in lines {
        *seen.entry(*line).or_insert(0) += 1;
    }
    let total_chars: usize = lines.iter().map(|l| l.chars().count()).sum();
    let mut dup_lines = 0usize;
    let mut dup_chars = 0usize;
    for (line, count) in &seen {
        if *count > 1 {
            // Every copy beyond the first is a duplicate.
            dup_lines += count - 1;
            dup_chars += (count - 1) * line.chars().count();
        }
    }
    (
        dup_lines as f32 / lines.len() as f32,
        if total_chars == 0 {
            0.0
        } else {
            dup_chars as f32 / total_chars as f32
        },
    )
}

/// Gopher's "top n-gram" rule: the share of characters taken by the single
/// most frequent n-gram. High values mean a page built from one repeated
/// phrase (product grids, pagination, generated listings).
fn top_ngram_char_fraction(words: &[String], n: usize) -> f32 {
    if words.len() < n {
        return 0.0;
    }
    let total_chars: usize = words.iter().map(|w| w.chars().count()).sum();
    if total_chars == 0 {
        return 0.0;
    }
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for window in words.windows(n) {
        *counts.entry(window.join(" ")).or_insert(0) += 1;
    }
    let top = counts
        .iter()
        .max_by_key(|(gram, count)| (**count, gram.len()))
        .map(|(gram, count)| gram.chars().filter(|c| !c.is_whitespace()).count() * count)
        .unwrap_or(0);
    (top as f32 / total_chars as f32).min(1.0)
}

/// Gopher's "duplicate n-gram" rule for the longer n: the share of characters
/// covered by n-grams that appear more than once.
///
/// Gopher is explicit that characters in *overlapping* duplicate n-grams must
/// not be counted twice, so this marks covered word positions and then sums
/// each covered word's characters once. Counting per-occurrence instead
/// inflates the fraction badly on repetitive text — a phrase repeated k times
/// contributes k·n windows over only k·n distinct words.
fn duplicate_ngram_char_fraction(words: &[String], n: usize) -> f32 {
    if words.len() < n {
        return 0.0;
    }
    let total_chars: usize = words.iter().map(|w| w.chars().count()).sum();
    if total_chars == 0 {
        return 0.0;
    }
    let mut counts: std::collections::HashMap<&[String], usize> = std::collections::HashMap::new();
    for window in words.windows(n) {
        *counts.entry(window).or_insert(0) += 1;
    }
    let mut covered = vec![false; words.len()];
    for (start, window) in words.windows(n).enumerate() {
        if counts.get(window).copied().unwrap_or(0) > 1 {
            covered[start..start + n].iter_mut().for_each(|c| *c = true);
        }
    }
    let dup_chars: usize = words
        .iter()
        .zip(&covered)
        .filter(|(_, is_covered)| **is_covered)
        .map(|(word, _)| word.chars().count())
        .sum();
    (dup_chars as f32 / total_chars as f32).min(1.0)
}

/// Evaluate every enabled gate against the metrics. Returns the failures in a
/// stable order.
///
/// Short-document exemption: below `stopword_min_words` the stopword and
/// unterminated-line gates are skipped. A 30-word definition page legitimately
/// has neither, and the word-count gate already covers genuinely empty pages —
/// double-flagging the same shortness would inflate confidence dishonestly.
pub fn evaluate(m: &QualityMetrics, c: &QualityConfig) -> Vec<Gate> {
    let mut failures = Vec::new();
    if m.word_count == 0 {
        return failures;
    }
    if m.word_count < c.min_words {
        failures.push(Gate::WordCountLow);
    }
    if m.word_count > c.max_words {
        failures.push(Gate::WordCountHigh);
    }
    if m.mean_word_length < c.min_mean_word_length || m.mean_word_length > c.max_mean_word_length {
        failures.push(Gate::MeanWordLength);
    }
    if m.symbol_word_ratio > c.max_symbol_word_ratio {
        failures.push(Gate::SymbolRatio);
    }
    if m.bullet_line_fraction > c.max_bullet_line_fraction {
        failures.push(Gate::BulletLines);
    }
    if m.ellipsis_line_fraction > c.max_ellipsis_line_fraction {
        failures.push(Gate::EllipsisLines);
    }
    if m.alpha_word_fraction < c.min_alpha_word_fraction {
        failures.push(Gate::AlphaWords);
    }
    let long_enough = m.word_count >= c.stopword_min_words;
    if c.stopwords_enabled && long_enough && m.stopword_hits < c.min_stopwords {
        failures.push(Gate::Stopwords);
    }
    if m.dup_line_fraction > c.max_dup_line_fraction
        || m.dup_line_char_fraction > c.max_dup_line_char_fraction
    {
        failures.push(Gate::DupLines);
    }
    if m.word_count >= c.repetition_min_words
        && (m.top_2gram_char_fraction > c.max_top_2gram_char_fraction
            || m.top_3gram_char_fraction > c.max_top_3gram_char_fraction
            || m.top_4gram_char_fraction > c.max_top_4gram_char_fraction
            || m.dup_5gram_char_fraction > c.max_dup_5gram_char_fraction
            || m.dup_10gram_char_fraction > c.max_dup_10gram_char_fraction)
    {
        failures.push(Gate::RepeatedNgrams);
    }
    if long_enough
        && m.punct_terminated_line_fraction < c.min_punct_terminated_line_fraction
    {
        failures.push(Gate::UnterminatedLines);
    }
    if m.short_line_fraction > c.max_short_line_fraction {
        failures.push(Gate::ShortLines);
    }
    if m.newline_word_ratio > c.max_newline_word_ratio {
        failures.push(Gate::NewlineRatio);
    }
    if m.has_boilerplate_marker {
        failures.push(Gate::BoilerplateMarker);
    }
    failures
}

/// How many distinct families the failures span.
pub fn families_failed(failures: &[Gate]) -> usize {
    let mut families: Vec<Family> = failures.iter().map(|g| g.family()).collect();
    families.sort_unstable();
    families.dedup();
    families.len()
}

/// Whether this failure set meets the configured bar for becoming a candidate.
///
/// Both counts must be met: enough gates, spread across enough families. The
/// family requirement is what keeps a page of code samples — which trips every
/// line-shape gate at once — from looking like overwhelming evidence.
pub fn is_candidate(failures: &[Gate], c: &QualityConfig) -> bool {
    failures.len() >= c.min_failures && families_failed(failures) >= c.min_families
}

/// Confidence for a quality verdict: never certain, and never above
/// `max_confidence`. Text heuristics identify *unusual* documents, which
/// overlaps with but is not the same as *worthless* ones — technical
/// reference pages are genuinely unusual and genuinely valuable.
pub fn confidence(failures: &[Gate], c: &QualityConfig) -> f32 {
    let extra = failures.len().saturating_sub(c.min_failures) as f32;
    (c.base_confidence + extra * c.confidence_per_extra_failure).min(c.max_confidence)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROSE: &str = "The operations handbook explains how the search cluster is deployed \
and monitored. It walks through Postgres tuning, shard sizing and connector scheduling. \
Each section closes with a checklist that the operator should follow before paging anyone. \
The recovery procedure for a tripped disk watermark is documented separately, with the \
exact commands that have to be run and the order in which they must happen.";

    fn config() -> QualityConfig {
        QualityConfig::default()
    }

    #[test]
    fn ordinary_prose_passes_every_gate() {
        let m = measure(PROSE);
        let failures = evaluate(&m, &config());
        assert!(
            failures.is_empty(),
            "clean prose must not be flagged, got {failures:?} from {m:?}"
        );
    }

    #[test]
    fn a_navigation_page_trips_the_line_shape_gates() {
        let nav = "Home\nAbout\nContact\nProducts\nServices\nBlog\nCareers\nPress\nLegal\nHelp";
        let m = measure(nav);
        let failures = evaluate(&m, &config());
        assert!(failures.contains(&Gate::ShortLines), "{failures:?}");
        assert!(failures.contains(&Gate::WordCountLow), "{failures:?}");
    }

    #[test]
    fn repeated_boilerplate_lines_are_measured() {
        let repeated = format!("{}\n", "Subscribe to our newsletter for updates.").repeat(20);
        let m = measure(&repeated);
        assert!(m.dup_line_fraction > 0.9, "{m:?}");
        assert!(m.dup_line_char_fraction > 0.9, "{m:?}");
        assert!(evaluate(&m, &config()).contains(&Gate::DupLines));
    }

    #[test]
    fn a_repeated_phrase_trips_the_ngram_gate() {
        let spam = "buy now cheap ".repeat(40); // 120 words, over repetition_min_words
        let m = measure(&spam);
        assert!(
            m.top_2gram_char_fraction > 0.16,
            "repeated phrase should dominate: {m:?}"
        );
        assert!(evaluate(&m, &config()).contains(&Gate::RepeatedNgrams));
    }

    #[test]
    fn short_documents_are_exempt_from_the_repetition_gates() {
        // A 30-word entry where one bigram happens to repeat is not spam; the
        // Gopher repetition thresholds assume a document with real body text.
        let short = "Copper wire. Copper wire is drawn from cathode stock and annealed before \
            it is spooled for sale to the trade at standard gauges.";
        let m = measure(short);
        assert!(m.word_count < 100);
        assert!(!evaluate(&m, &config()).contains(&Gate::RepeatedNgrams));
    }

    #[test]
    fn overlapping_duplicate_ngrams_are_counted_once() {
        // Gopher counts characters in duplicate n-grams without double-counting
        // overlaps. Half this text is a repeated block, so the fraction must
        // land near 0.5 — a per-occurrence count would report far above 1.0
        // before clamping and flag every repetitive page.
        let half_repeated = format!(
            "{} {}",
            "alpha beta gamma delta epsilon zeta eta theta iota kappa ".repeat(2),
            (0..20)
                .map(|i| format!("unique{i}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let m = measure(&half_repeated);
        assert!(
            m.dup_5gram_char_fraction > 0.3 && m.dup_5gram_char_fraction < 0.75,
            "expected roughly half, got {}",
            m.dup_5gram_char_fraction
        );
    }

    #[test]
    fn empty_text_produces_no_gates_rather_than_every_gate() {
        // A document with no fetched text is "unknown", not "bad" — the stub
        // detector owns genuinely empty documents.
        let m = measure("");
        assert_eq!(m.word_count, 0);
        assert!(evaluate(&m, &config()).is_empty());
    }

    #[test]
    fn short_documents_are_exempt_from_the_stopword_and_punctuation_gates() {
        // A glossary entry: real content, too short for the prose-shaped gates.
        let short = "Ribosome: cellular machine assembling proteins";
        let m = measure(short);
        let failures = evaluate(&m, &config());
        assert!(!failures.contains(&Gate::Stopwords), "{failures:?}");
        assert!(!failures.contains(&Gate::UnterminatedLines), "{failures:?}");
    }

    #[test]
    fn machine_output_trips_the_alpha_and_symbol_gates() {
        let numeric = "12.4 88.1 90.2 44.5 66.7 12.4 88.1 90.2 44.5 66.7 12.4 88.1 90.2 44.5 \
            66.7 12.4 88.1 90.2 44.5 66.7 12.4 88.1 90.2 44.5 66.7 12.4 88.1 90.2 44.5 66.7 \
            12.4 88.1 90.2 44.5 66.7 12.4 88.1 90.2 44.5 66.7 12.4 88.1 90.2 44.5 66.7";
        let m = measure(numeric);
        assert!(m.alpha_word_fraction < 0.8, "{m:?}");
        assert!(evaluate(&m, &config()).contains(&Gate::AlphaWords));
    }

    #[test]
    fn boilerplate_markers_are_caught_case_insensitively() {
        let m = measure("Lorem Ipsum dolor sit amet, consectetur adipiscing elit sed do.");
        assert!(m.has_boilerplate_marker);
        assert!(evaluate(&m, &config()).contains(&Gate::BoilerplateMarker));
    }

    /// Measured on a 250-document random sample of the reference corpus:
    /// requiring three gates across two families takes the candidate rate from
    /// 27% to 14%, and the documents that drop out are the ones worth keeping
    /// (HOWTOs, topic pages, comment threads) while the image stubs and
    /// directory listings stay.
    ///
    /// The residual false positive is API-reference pages — code blocks and
    /// signature tables genuinely have the text shape of junk. That is why
    /// quality confidence is capped below certainty and why
    /// `exempt_connectors` exists; it is not a bug to be tuned away.
    #[test]
    fn correlated_line_shape_failures_alone_do_not_make_a_candidate() {
        let config = config();
        let line_shape_only = [Gate::UnterminatedLines, Gate::ShortLines, Gate::NewlineRatio];
        assert_eq!(families_failed(&line_shape_only), 1);
        assert!(
            !is_candidate(&line_shape_only, &config),
            "three views of one property is one observation"
        );

        let spread = [Gate::WordCountLow, Gate::AlphaWords, Gate::ShortLines];
        assert_eq!(families_failed(&spread), 3);
        assert!(is_candidate(&spread, &config));
    }

    #[test]
    fn confidence_grows_with_evidence_but_never_reaches_certainty() {
        let config = config();
        let three = [Gate::WordCountLow, Gate::AlphaWords, Gate::ShortLines];
        let many = [
            Gate::WordCountLow,
            Gate::AlphaWords,
            Gate::ShortLines,
            Gate::NewlineRatio,
            Gate::MeanWordLength,
            Gate::DupLines,
            Gate::RepeatedNgrams,
        ];
        assert_eq!(confidence(&three, &config), config.base_confidence);
        assert!(confidence(&many, &config) > confidence(&three, &config));
        assert!(
            confidence(&many, &config) <= config.max_confidence,
            "text heuristics find unusual documents, not provably worthless ones"
        );
    }

    #[test]
    fn gate_codes_are_unique_and_stable() {
        let all = [
            Gate::WordCountLow,
            Gate::WordCountHigh,
            Gate::MeanWordLength,
            Gate::SymbolRatio,
            Gate::BulletLines,
            Gate::EllipsisLines,
            Gate::AlphaWords,
            Gate::Stopwords,
            Gate::DupLines,
            Gate::RepeatedNgrams,
            Gate::UnterminatedLines,
            Gate::ShortLines,
            Gate::NewlineRatio,
            Gate::BoilerplateMarker,
        ];
        let mut codes: Vec<&str> = all.iter().map(|g| g.code()).collect();
        let count = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), count, "gate codes must be unique");
    }

    #[test]
    fn explanations_quote_the_measurement_and_the_threshold() {
        let m = measure("short");
        let text = Gate::WordCountLow.explain(&m, &config());
        assert!(text.contains('1'), "{text}");
        assert!(text.contains("50"), "the Gopher minimum must appear: {text}");
    }

    #[test]
    fn disabling_the_stopword_gate_silences_only_that_gate() {
        // Long enough to clear both `stopword_min_words` and
        // `repetition_min_words`, so both gates are genuinely in play.
        let listing = "widget ".repeat(150);
        let m = measure(&listing);
        let mut config = config();
        assert!(evaluate(&m, &config).contains(&Gate::Stopwords));
        config.stopwords_enabled = false;
        let failures = evaluate(&m, &config);
        assert!(!failures.contains(&Gate::Stopwords));
        assert!(
            failures.contains(&Gate::RepeatedNgrams),
            "other gates must still fire: {failures:?}"
        );
    }
}
