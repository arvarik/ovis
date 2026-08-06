//! MinHash/LSH near-duplicate detection — the tested algorithmic core kept
//! from the original crate, fed by the API-era data layer.
//!
//! Two ways to use it:
//!
//! * [`MinHashDedupEngine::detect_duplicates`] over an in-memory corpus — fine
//!   for scoped scans (a connector's documents fit comfortably in memory).
//! * The streaming pieces ([`compute_signature`](MinHashDedupEngine::compute_signature),
//!   [`band_hashes`](MinHashDedupEngine::band_hashes),
//!   [`jaccard_similarity`](MinHashDedupEngine::jaccard_similarity)) for the
//!   checkpointed full-corpus scan, where signatures and band buckets are
//!   persisted between pages rather than held in one big Vec.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::config::{MinHashConfig, PreferKeepPolicy};

/// A document with reconstructed chunk text, as the detectors consume it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocumentWithContent {
    pub id: String,
    pub semantic_id: String,
    pub connector_id: Option<i32>,
    pub link: Option<String>,
    pub content: String,
    pub chunk_count: Option<i32>,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// A detected duplicate pair with similarity and the keeper decision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DuplicatePair {
    pub kept_document_id: String,
    pub duplicate_document_id: String,
    pub jaccard_similarity: f64,
    /// Human-readable keeper rationale, e.g. "shortest URL (34 vs 61 chars)".
    pub reason: String,
}

/// Tokenize raw text into word k-shingles.
pub fn shingle_text(text: &str, k: usize) -> Vec<String> {
    let words: Vec<&str> = text
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .collect();

    if words.is_empty() {
        return Vec::new();
    }

    if words.len() <= k {
        return vec![words.join(" ").to_lowercase()];
    }

    let mut shingles = Vec::with_capacity(words.len() - k + 1);
    for window in words.windows(k) {
        shingles.push(window.join(" ").to_lowercase());
    }
    shingles
}

/// Deterministic FNV-1a 64-bit hash of a string shingle.
fn hash_shingle(shingle: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in shingle.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Deterministic FNV-1a 64-bit hash of a u64 slice (for band hashing).
fn hash_slice(slice: &[u64]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &val in slice {
        for byte in val.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

/// Coefficients for the permutation hash h_i(x) = a_i·x + b_i (mod 2^64).
#[derive(Debug, Clone, Copy)]
struct HashCoeff {
    a: u64,
    b: u64,
}

/// MinHash LSH deduplication engine.
///
/// Coefficients are generated from a fixed seed, so signatures are stable
/// across processes and restarts — which is what makes persisted signatures
/// resumable.
pub struct MinHashDedupEngine {
    config: MinHashConfig,
    coeffs: Vec<HashCoeff>,
}

impl MinHashDedupEngine {
    pub fn new(config: MinHashConfig) -> Self {
        let num_perm = config.num_perm;
        let mut coeffs = Vec::with_capacity(num_perm);

        let mut seed: u64 = 0x4D696E4861736831; // "MinHash1"
        for _ in 0..num_perm {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let mut a = seed;
            if a.is_multiple_of(2) {
                a = a.wrapping_add(1); // odd ⇒ coprime with 2^64
            }

            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let b = seed;

            coeffs.push(HashCoeff { a, b });
        }

        Self { config, coeffs }
    }

    pub fn config(&self) -> &MinHashConfig {
        &self.config
    }

    /// Number of LSH bands (config override or 16).
    pub fn num_bands(&self) -> usize {
        self.config.bands.unwrap_or(16).max(1)
    }

    /// MinHash signature for a set of shingles. `num_perm` u64s.
    pub fn compute_signature(&self, shingles: &[String]) -> Vec<u64> {
        let num_perm = self.config.num_perm;
        let mut sig = vec![u64::MAX; num_perm];

        if shingles.is_empty() {
            return sig;
        }

        for shingle in shingles {
            let x = hash_shingle(shingle);
            for (slot, coeff) in sig.iter_mut().zip(&self.coeffs[..num_perm]) {
                let h = coeff.a.wrapping_mul(x).wrapping_add(coeff.b);
                if h < *slot {
                    *slot = h;
                }
            }
        }

        sig
    }

    /// Shorthand: signature straight from text.
    pub fn signature_for_text(&self, text: &str) -> Vec<u64> {
        self.compute_signature(&shingle_text(text, self.config.shingle_size))
    }

    /// The per-band bucket hashes for a signature — two documents sharing any
    /// band hash are a candidate pair.
    pub fn band_hashes(&self, sig: &[u64]) -> Vec<u64> {
        let num_bands = self.num_bands();
        let rows_per_band = (self.config.num_perm / num_bands).max(1);
        let mut hashes = Vec::with_capacity(num_bands);
        for b in 0..num_bands {
            let start = b * rows_per_band;
            let end = ((b + 1) * rows_per_band).min(sig.len());
            if start < end {
                hashes.push(hash_slice(&sig[start..end]));
            }
        }
        hashes
    }

    /// Estimated Jaccard similarity between two signatures.
    pub fn jaccard_similarity(&self, sig1: &[u64], sig2: &[u64]) -> f64 {
        if sig1.len() != sig2.len() || sig1.is_empty() {
            return 0.0;
        }

        let mut matches = 0usize;
        for i in 0..sig1.len() {
            if sig1[i] == sig2[i] && sig1[i] != u64::MAX {
                matches += 1;
            }
        }

        matches as f64 / sig1.len() as f64
    }

    /// Detect duplicate pairs across an in-memory document set.
    ///
    /// Emits one pair per flagged duplicate (a document is flagged at most
    /// once, against its chosen keeper), at or above
    /// `config.jaccard_threshold`.
    pub fn detect_duplicates(
        &self,
        docs: &[DocumentWithContent],
        prefer_keep: PreferKeepPolicy,
    ) -> Vec<DuplicatePair> {
        if docs.len() < 2 {
            return Vec::new();
        }

        let signatures: Vec<Vec<u64>> = docs
            .iter()
            .map(|doc| self.signature_for_text(&doc.content))
            .collect();

        // LSH band bucketing: (band index, band hash) -> doc indices.
        let mut buckets: HashMap<(usize, u64), Vec<usize>> = HashMap::new();
        for (doc_idx, sig) in signatures.iter().enumerate() {
            for (band, hash) in self.band_hashes(sig).into_iter().enumerate() {
                buckets.entry((band, hash)).or_default().push(doc_idx);
            }
        }

        let mut candidate_pair_indices = HashSet::new();
        for bucket_docs in buckets.values() {
            if bucket_docs.len() >= 2 {
                for i in 0..bucket_docs.len() {
                    for j in (i + 1)..bucket_docs.len() {
                        let (a, b) = (bucket_docs[i], bucket_docs[j]);
                        candidate_pair_indices.insert(if a < b { (a, b) } else { (b, a) });
                    }
                }
            }
        }

        let mut pairs = Vec::new();
        let mut flagged = HashSet::new();

        let mut ordered: Vec<(usize, usize)> = candidate_pair_indices.into_iter().collect();
        ordered.sort_unstable();

        for (idx1, idx2) in ordered {
            let sim = self.jaccard_similarity(&signatures[idx1], &signatures[idx2]);
            if sim >= self.config.jaccard_threshold {
                let (kept, duplicate, reason) =
                    select_keep_document(&docs[idx1], &docs[idx2], prefer_keep);

                if flagged.insert(duplicate.id.clone()) {
                    pairs.push(DuplicatePair {
                        kept_document_id: kept.id.clone(),
                        duplicate_document_id: duplicate.id.clone(),
                        jaccard_similarity: sim,
                        reason,
                    });
                }
            }
        }

        pairs
    }
}

/// Pick which of two documents survives, per policy. Ties break toward the
/// lexicographically smaller id, so the choice is deterministic.
pub fn select_keep_document<'a>(
    doc1: &'a DocumentWithContent,
    doc2: &'a DocumentWithContent,
    policy: PreferKeepPolicy,
) -> (&'a DocumentWithContent, &'a DocumentWithContent, String) {
    let (keep_first, reason) = match policy {
        PreferKeepPolicy::LongestContent => {
            let (l1, l2) = (doc1.content.len(), doc2.content.len());
            (
                l1 > l2 || (l1 == l2 && doc1.id <= doc2.id),
                format!("longest content ({l1} vs {l2} chars)"),
            )
        }
        PreferKeepPolicy::NewestUpdated => {
            let (t1, t2) = (doc1.updated_at, doc2.updated_at);
            (
                t1 > t2 || (t1 == t2 && doc1.id <= doc2.id),
                format!("newest updated ({t1:?} vs {t2:?})"),
            )
        }
        PreferKeepPolicy::ShortestUrl => {
            let len = |d: &DocumentWithContent| {
                d.link.as_deref().map(str::len).unwrap_or(d.id.len())
            };
            let (l1, l2) = (len(doc1), len(doc2));
            (
                l1 < l2 || (l1 == l2 && doc1.id <= doc2.id),
                format!("shortest URL ({l1} vs {l2} chars)"),
            )
        }
        PreferKeepPolicy::MostChunks => {
            let (c1, c2) = (
                doc1.chunk_count.unwrap_or(0),
                doc2.chunk_count.unwrap_or(0),
            );
            (
                c1 > c2 || (c1 == c2 && doc1.id <= doc2.id),
                format!("most chunks ({c1} vs {c2})"),
            )
        }
    };

    if keep_first {
        (doc1, doc2, reason)
    } else {
        (doc2, doc1, reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_doc(id: &str, title: &str, content: &str) -> DocumentWithContent {
        DocumentWithContent {
            id: id.to_string(),
            semantic_id: title.to_string(),
            connector_id: Some(1),
            link: Some(format!("https://example.com/{id}")),
            content: content.to_string(),
            chunk_count: None,
            updated_at: None,
        }
    }

    #[test]
    fn test_shingle_text_5_gram() {
        let text = "The quick brown fox jumps over the lazy dog in the garden";
        let shingles = shingle_text(text, 5);
        assert_eq!(shingles.len(), 8);
        assert_eq!(shingles[0], "the quick brown fox jumps");
        assert_eq!(shingles[1], "quick brown fox jumps over");
    }

    #[test]
    fn test_shingle_text_short_text() {
        let shingles = shingle_text("Hello world", 5);
        assert_eq!(shingles.len(), 1);
        assert_eq!(shingles[0], "hello world");
    }

    #[test]
    fn identical_texts_have_identical_signatures() {
        let engine = MinHashDedupEngine::new(MinHashConfig::default());
        let text = "This is a detailed technical document about distributed storage systems and consensus algorithms such as Raft and Paxos.";
        let sig1 = engine.signature_for_text(text);
        let sig2 = engine.signature_for_text(text);
        assert_eq!(sig1, sig2);
        assert_eq!(engine.jaccard_similarity(&sig1, &sig2), 1.0);
    }

    #[test]
    fn signatures_are_stable_across_engine_instances() {
        // Persisted signatures must survive a restart: two engines built from
        // the same config produce the same signature for the same text.
        let a = MinHashDedupEngine::new(MinHashConfig::default());
        let b = MinHashDedupEngine::new(MinHashConfig::default());
        let text = "Stability across restarts is what makes the checkpointed scan resumable.";
        assert_eq!(a.signature_for_text(text), b.signature_for_text(text));
        assert_eq!(
            a.band_hashes(&a.signature_for_text(text)),
            b.band_hashes(&b.signature_for_text(text))
        );
    }

    #[test]
    fn band_hashes_cover_the_configured_band_count() {
        let engine = MinHashDedupEngine::new(MinHashConfig::default());
        let sig = engine.signature_for_text("some text to hash into a full signature vector");
        assert_eq!(engine.band_hashes(&sig).len(), engine.num_bands());
    }

    #[test]
    fn near_duplicates_detected_and_unrelated_text_is_not() {
        let engine = MinHashDedupEngine::new(MinHashConfig {
            jaccard_threshold: 0.85,
            ..MinHashConfig::default()
        });

        let base_text = "This is a comprehensive guide to building resilient distributed systems with Rust Tokio and PostgreSQL storage layer. It covers memory safety, async runtimes, connection pools, and query optimization techniques in depth. The architecture incorporates heuristic quality filters, character count evaluation, regex matching, URL blacklists, locality sensitive hashing, and structured JSON audit reporting for automated repository maintenance and data curation.";
        let modified_text = "This is a comprehensive guide to building resilient distributed systems with Rust Tokio and PostgreSQL storage layer. It covers memory safety, async runtimes, connection pools, and query optimization techniques in depth. The architecture incorporates heuristic quality filters, character count evaluation, regex matching, URL blacklists, locality sensitive hashing, and structured JSON audit reporting for automated repository maintenance and quality data curation.";

        let docs = vec![
            make_doc("doc1", "Distributed Systems Guide", base_text),
            make_doc("doc2", "Distributed Systems Guide (Copy)", modified_text),
            make_doc("doc3", "Unrelated Topic", "Quantum computing leverages qubits and entanglement to perform complex matrix calculations exponentially faster than classical supercomputers."),
        ];
        let pairs = engine.detect_duplicates(&docs, PreferKeepPolicy::LongestContent);

        assert_eq!(pairs.len(), 1);
        assert!(pairs[0].jaccard_similarity >= 0.85);
        assert_eq!(pairs[0].duplicate_document_id, "doc1");
        assert_eq!(pairs[0].kept_document_id, "doc2");
    }

    #[test]
    fn keeper_policies_choose_deterministically() {
        let mut a = make_doc("a", "A", "short");
        let mut b = make_doc("b", "B", "much longer content here");
        a.link = Some("https://example.com/a".into());
        b.link = Some("https://example.com/a/very/deep/path".into());
        a.chunk_count = Some(2);
        b.chunk_count = Some(9);

        let (kept, dup, _) = select_keep_document(&a, &b, PreferKeepPolicy::ShortestUrl);
        assert_eq!((kept.id.as_str(), dup.id.as_str()), ("a", "b"));

        let (kept, _, _) = select_keep_document(&a, &b, PreferKeepPolicy::LongestContent);
        assert_eq!(kept.id, "b");

        let (kept, _, _) = select_keep_document(&a, &b, PreferKeepPolicy::MostChunks);
        assert_eq!(kept.id, "b");

        // Tie on the criterion → smaller id wins, both directions.
        let c = make_doc("c", "C", "same length!");
        let d = make_doc("d", "D", "same length!");
        let (kept1, _, _) = select_keep_document(&c, &d, PreferKeepPolicy::LongestContent);
        let (kept2, _, _) = select_keep_document(&d, &c, PreferKeepPolicy::LongestContent);
        assert_eq!(kept1.id, kept2.id);
    }
}
