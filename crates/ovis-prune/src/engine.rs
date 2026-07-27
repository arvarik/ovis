use std::collections::HashMap;
use crate::config::PruneConfig;
use crate::dedup::MinHashDedupEngine;
use crate::heuristics::{DocumentWithContent, HeuristicEvaluator, PruneCandidate};
use crate::reporter::PruneAuditReport;

/// High-performance multi-stage Pruning Engine combining fast microsecond heuristics and MinHash LSH deduplication.
pub struct PruningEngine {
    config: PruneConfig,
    heuristic_evaluator: HeuristicEvaluator,
    dedup_engine: MinHashDedupEngine,
}

impl PruningEngine {
    /// Construct a new `PruningEngine` with a parsed `PruneConfig`.
    pub fn new(config: PruneConfig) -> anyhow::Result<Self> {
        let heuristic_evaluator = HeuristicEvaluator::new(config.heuristics.clone())?;
        let dedup_engine = MinHashDedupEngine::new(config.deduplication.minhash.clone());
        Ok(Self {
            config,
            heuristic_evaluator,
            dedup_engine,
        })
    }

    /// Access inner configuration.
    pub fn config(&self) -> &PruneConfig {
        &self.config
    }

    /// Evaluate a collection of repository documents through all active pruning pipeline stages.
    pub fn evaluate_repository(
        &self,
        docs: &[DocumentWithContent],
    ) -> anyhow::Result<PruneAuditReport> {
        let mut candidates_map: HashMap<String, PruneCandidate> = HashMap::new();

        // Stage 1: Fast microsecond Heuristic Pre-Filtering
        for doc in docs {
            let reasons = self.heuristic_evaluator.evaluate(doc);
            if !reasons.is_empty() {
                candidates_map.insert(
                    doc.id.clone(),
                    PruneCandidate {
                        document_id: doc.id.clone(),
                        title: doc.semantic_id.clone(),
                        connector_id: doc.connector_id,
                        flag_reasons: reasons,
                        duplicate_of: None,
                    },
                );
            }
        }

        // Stage 2: MinHash LSH Near-Deduplication Engine
        let mut duplicate_pairs = Vec::new();
        if self.config.deduplication.enabled {
            let (dups, candidates_from_dedup) = self
                .dedup_engine
                .detect_duplicates(docs, &self.config.deduplication.prefer_keep);
            duplicate_pairs = dups;

            for candidate in candidates_from_dedup {
                candidates_map
                    .entry(candidate.document_id.clone())
                    .and_modify(|existing| {
                        existing.flag_reasons.extend(candidate.flag_reasons.clone());
                        if existing.duplicate_of.is_none() {
                            existing.duplicate_of = candidate.duplicate_of.clone();
                        }
                    })
                    .or_insert(candidate);
            }
        }

        let candidates: Vec<PruneCandidate> = candidates_map.into_values().collect();
        let total_candidates_flagged = candidates.len();
        let total_duplicates_detected = duplicate_pairs.len();

        let scope = self
            .config
            .repository_scope
            .clone()
            .unwrap_or_else(|| "Default Repository Scope".to_string());

        let report = PruneAuditReport {
            timestamp: chrono::Utc::now(),
            total_documents_evaluated: docs.len(),
            total_candidates_flagged,
            total_duplicates_detected,
            dry_run: self.config.execution.dry_run,
            scope,
            candidates,
            duplicate_pairs,
        };

        Ok(report)
    }

    /// Execute physical cascading page deletion for flagged candidates if dry-run is disabled or auto_delete is enabled.
    ///
    /// Deletion goes through `ovis_core::db::documents::delete_document_cascading`,
    /// which sweeps every foreign-key child of `document` and queues a failed
    /// index cleanup for retry. `index` must be the live index name from
    /// `search_settings` — never a `danswer_chunk*` wildcard.
    pub async fn execute_pruning(
        &self,
        pg_pool: &sqlx::PgPool,
        os: &ovis_core::search::OsClient,
        index: &str,
        report: &PruneAuditReport,
    ) -> anyhow::Result<usize> {
        if report.dry_run && !self.config.execution.auto_delete {
            tracing::info!("Dry-run mode active. Skipping physical database deletion.");
            return Ok(0);
        }

        let mut deleted_count = 0;
        for candidate in &report.candidates {
            match ovis_core::db::documents::delete_document_cascading(
                pg_pool,
                os,
                index,
                &candidate.document_id,
            )
            .await
            {
                Ok(outcome) => {
                    tracing::info!(
                        doc_id = %candidate.document_id,
                        chunks_deleted = outcome.chunks_deleted,
                        index_cleanup_pending = outcome.index_cleanup_pending,
                        "PruningEngine cascading page deletion successful"
                    );
                    deleted_count += 1;
                }
                Err(e) => {
                    tracing::warn!(
                        doc_id = %candidate.document_id,
                        error = %e,
                        "Cascading page deletion skipped or failed"
                    );
                }
            }
        }

        Ok(deleted_count)
    }
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
    fn test_engine_pipeline_evaluation() {
        let config = PruneConfig::default();
        let engine = PruningEngine::new(config).expect("Engine initialization failed");

        let doc_stub = make_doc("doc_stub", "Short Stub", None, "Short text");
        let doc_valid = make_doc(
            "doc_valid",
            "Valid Tech Specs",
            Some("https://docs.company.internal/specs"),
            "This is a high quality technical specification document explaining the system design of the pruning engine in detail. It includes comprehensive descriptions of heuristics, MinHash LSH deduplication, configuration management, and audit reporting.",
        );
        let doc_dup = make_doc(
            "doc_dup",
            "Valid Tech Specs Copy",
            Some("https://docs.company.internal/specs_copy"),
            "This is a high quality technical specification document explaining the system design of the pruning engine in detail. It includes comprehensive descriptions of heuristics, MinHash LSH deduplication, configuration management, and audit reporting with minor word updates.",
        );

        let docs = vec![doc_stub, doc_valid, doc_dup];
        let report = engine.evaluate_repository(&docs).expect("Evaluation failed");

        assert_eq!(report.total_documents_evaluated, 3);
        assert!(report.total_candidates_flagged >= 2);
        assert!(report.total_duplicates_detected >= 1);
        assert!(report.dry_run);
    }
}
