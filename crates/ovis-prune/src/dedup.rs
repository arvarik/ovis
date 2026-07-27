use std::collections::{HashMap, HashSet};
use serde::{Deserialize, Serialize};

use crate::config::{MinHashConfig, PreferKeepPolicy};
use crate::heuristics::{DocumentWithContent, PruneCandidate, PruneFlagReason};

/// Represents a detected duplicate document pair with similarity metric and retention reasoning.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DuplicatePair {
    pub kept_document_id: String,
    pub duplicate_document_id: String,
    pub jaccard_similarity: f64,
    pub reason: String,
}

/// Tokenize raw text into word k-shingles (n-grams).
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

/// Compute deterministic FNV-1a 64-bit hash of a string shingle.
fn hash_shingle(shingle: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in shingle.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Compute deterministic FNV-1a 64-bit hash of a u64 slice (for band hashing).
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

/// Pair of coefficients for MinHash permutation hash functions: h_i(x) = (a_i * x + b_i) mod 2^64
#[derive(Debug, Clone, Copy)]
struct HashCoeff {
    a: u64,
    b: u64,
}

/// MinHash LSH deduplication engine providing O(N) near-duplicate document identification.
pub struct MinHashDedupEngine {
    config: MinHashConfig,
    coeffs: Vec<HashCoeff>,
}

impl MinHashDedupEngine {
    /// Initialize `MinHashDedupEngine` with deterministic permutation hash coefficients.
    pub fn new(config: MinHashConfig) -> Self {
        let num_perm = config.num_perm;
        let mut coeffs = Vec::with_capacity(num_perm);

        // Generate deterministic pseudo-random (a, b) coefficient pairs using LCG
        let mut seed: u64 = 0x4D696E4861736831; // "MinHash1"
        for _ in 0..num_perm {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let mut a = seed;
            if a % 2 == 0 {
                a = a.wrapping_add(1); // Ensure odd integer for coprimality with 2^64
            }

            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let b = seed;

            coeffs.push(HashCoeff { a, b });
        }

        Self { config, coeffs }
    }

    /// Compute 128-element MinHash signature vector for a set of shingles.
    pub fn compute_signature(&self, shingles: &[String]) -> Vec<u64> {
        let num_perm = self.config.num_perm;
        let mut sig = vec![u64::MAX; num_perm];

        if shingles.is_empty() {
            return sig;
        }

        for shingle in shingles {
            let x = hash_shingle(shingle);
            for i in 0..num_perm {
                let h = self.coeffs[i].a.wrapping_mul(x).wrapping_add(self.coeffs[i].b);
                if h < sig[i] {
                    sig[i] = h;
                }
            }
        }

        sig
    }

    /// Calculate MinHash Jaccard similarity estimate between two signature vectors.
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

    /// Detect duplicate document pairs across a slice of documents using LSH band bucketing.
    pub fn detect_duplicates(
        &self,
        docs: &[DocumentWithContent],
        prefer_keep: &PreferKeepPolicy,
    ) -> (Vec<DuplicatePair>, Vec<PruneCandidate>) {
        if docs.len() < 2 {
            return (Vec::new(), Vec::new());
        }

        let num_perm = self.config.num_perm;
        let num_bands = self.config.bands.unwrap_or(16);
        let rows_per_band = num_perm / num_bands;

        // Compute MinHash signatures for all documents
        let signatures: Vec<Vec<u64>> = docs
            .iter()
            .map(|doc| {
                let shingles = shingle_text(&doc.content, self.config.shingle_size);
                self.compute_signature(&shingles)
            })
            .collect();

        // LSH Band Bucketing: (band_index, band_hash) -> Vec<doc_index>
        let mut buckets: HashMap<(usize, u64), Vec<usize>> = HashMap::new();
        for (doc_idx, sig) in signatures.iter().enumerate() {
            for b in 0..num_bands {
                let start = b * rows_per_band;
                let end = (b + 1) * rows_per_band;
                if end <= sig.len() {
                    let band_hash = hash_slice(&sig[start..end]);
                    buckets.entry((b, band_hash)).or_default().push(doc_idx);
                }
            }
        }

        // Collect candidate document index pairs (i < j)
        let mut candidate_pair_indices = HashSet::new();
        for bucket_docs in buckets.values() {
            if bucket_docs.len() >= 2 {
                for i in 0..bucket_docs.len() {
                    for j in (i + 1)..bucket_docs.len() {
                        let idx1 = bucket_docs[i];
                        let idx2 = bucket_docs[j];
                        let pair = if idx1 < idx2 { (idx1, idx2) } else { (idx2, idx1) };
                        candidate_pair_indices.insert(pair);
                    }
                }
            }
        }

        let mut duplicate_pairs = Vec::new();
        let mut prune_candidates = Vec::new();
        let mut flagged_as_duplicate = HashSet::new();

        for (idx1, idx2) in candidate_pair_indices {
            let sim = self.jaccard_similarity(&signatures[idx1], &signatures[idx2]);
            if sim >= self.config.jaccard_threshold {
                let doc1 = &docs[idx1];
                let doc2 = &docs[idx2];

                let (kept_doc, dup_doc, reason) = select_keep_document(doc1, doc2, prefer_keep, sim);

                if !flagged_as_duplicate.contains(&dup_doc.id) {
                    flagged_as_duplicate.insert(dup_doc.id.clone());

                    duplicate_pairs.push(DuplicatePair {
                        kept_document_id: kept_doc.id.clone(),
                        duplicate_document_id: dup_doc.id.clone(),
                        jaccard_similarity: sim,
                        reason: reason.clone(),
                    });

                    prune_candidates.push(PruneCandidate {
                        document_id: dup_doc.id.clone(),
                        title: dup_doc.semantic_id.clone(),
                        connector_id: dup_doc.connector_id,
                        flag_reasons: vec![PruneFlagReason {
                            rule_name: "minhash_lsh_deduplication".to_string(),
                            description: format!(
                                "Near-duplicate of '{}' (Jaccard similarity {:.4} >= threshold {:.4}, policy: {})",
                                kept_doc.id, sim, self.config.jaccard_threshold, reason
                            ),
                            confidence: sim as f32,
                        }],
                        duplicate_of: Some(kept_doc.id.clone()),
                    });
                }
            }
        }

        (duplicate_pairs, prune_candidates)
    }
}

/// Select which document to keep based on the `PreferKeepPolicy`.
fn select_keep_document<'a>(
    doc1: &'a DocumentWithContent,
    doc2: &'a DocumentWithContent,
    policy: &PreferKeepPolicy,
    _sim: f64,
) -> (&'a DocumentWithContent, &'a DocumentWithContent, String) {
    match policy {
        PreferKeepPolicy::LongestContent => {
            if doc1.content.len() >= doc2.content.len() {
                (
                    doc1,
                    doc2,
                    format!("Keep longest content ({} chars vs {} chars)", doc1.content.len(), doc2.content.len()),
                )
            } else {
                (
                    doc2,
                    doc1,
                    format!("Keep longest content ({} chars vs {} chars)", doc2.content.len(), doc1.content.len()),
                )
            }
        }
        PreferKeepPolicy::NewestUpdated => {
            let t1 = doc1.updated_at;
            let t2 = doc2.updated_at;
            if t1 >= t2 {
                (
                    doc1,
                    doc2,
                    format!("Keep newest updated timestamp ({:?} vs {:?})", t1, t2),
                )
            } else {
                (
                    doc2,
                    doc1,
                    format!("Keep newest updated timestamp ({:?} vs {:?})", t2, t1),
                )
            }
        }
        PreferKeepPolicy::ShortestUrl => {
            let len1 = doc1.link.as_ref().map(|l| l.len()).unwrap_or(doc1.id.len());
            let len2 = doc2.link.as_ref().map(|l| l.len()).unwrap_or(doc2.id.len());
            if len1 <= len2 {
                (
                    doc1,
                    doc2,
                    format!("Keep shortest URL ({} chars vs {} chars)", len1, len2),
                )
            } else {
                (
                    doc2,
                    doc1,
                    format!("Keep shortest URL ({} chars vs {} chars)", len2, len1),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_doc(id: &str, title: &str, content: &str) -> DocumentWithContent {
        DocumentWithContent {
            id: id.to_string(),
            semantic_id: title.to_string(),
            connector_id: 1,
            link: Some(format!("https://example.com/{}", id)),
            content: content.to_string(),
            updated_at: None,
            metadata: json!({}),
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
        let text = "Hello world";
        let shingles = shingle_text(text, 5);
        assert_eq!(shingles.len(), 1);
        assert_eq!(shingles[0], "hello world");
    }

    #[test]
    fn test_minhash_signature_identical_texts() {
        let engine = MinHashDedupEngine::new(MinHashConfig::default());
        let text = "This is a detailed technical document about distributed storage systems and consensus algorithms such as Raft and Paxos.";
        let shingles = shingle_text(text, 5);

        let sig1 = engine.compute_signature(&shingles);
        let sig2 = engine.compute_signature(&shingles);

        assert_eq!(sig1, sig2);
        let sim = engine.jaccard_similarity(&sig1, &sig2);
        assert_eq!(sim, 1.0);
    }

    #[test]
    fn test_minhash_near_duplicates_detected() {
        let engine = MinHashDedupEngine::new(MinHashConfig::default());

        let base_text = "This is a comprehensive guide to building resilient distributed systems with Rust Tokio and PostgreSQL storage layer. It covers memory safety, async runtimes, connection pools, and query optimization techniques in depth. The architecture incorporates heuristic quality filters, character count evaluation, regex matching, URL blacklists, locality sensitive hashing, and structured JSON audit reporting for automated repository maintenance and data curation.";
        let modified_text = "This is a comprehensive guide to building resilient distributed systems with Rust Tokio and PostgreSQL storage layer. It covers memory safety, async runtimes, connection pools, and query optimization techniques in depth. The architecture incorporates heuristic quality filters, character count evaluation, regex matching, URL blacklists, locality sensitive hashing, and structured JSON audit reporting for automated repository maintenance and quality data curation.";

        let doc1 = make_doc("doc1", "Distributed Systems Guide", base_text);
        let doc2 = make_doc("doc2", "Distributed Systems Guide (Copy)", modified_text);
        let doc3 = make_doc("doc3", "Unrelated Topic", "Quantum computing leverages qubits and entanglement to perform complex matrix calculations exponentially faster than classical supercomputers.");

        let docs = vec![doc1, doc2, doc3];
        let (dups, candidates) = engine.detect_duplicates(&docs, &PreferKeepPolicy::LongestContent);

        assert_eq!(dups.len(), 1);
        assert_eq!(candidates.len(), 1);
        assert!(dups[0].jaccard_similarity >= 0.85);
        assert_eq!(dups[0].duplicate_document_id, "doc1");
        assert_eq!(dups[0].kept_document_id, "doc2");
    }
}
