//! Naming duplicate clusters and detector bundles.
//!
//! This is the first thing in OVIS that sends corpus content to a model, and it
//! was chosen to be first precisely because it has no safety surface: it reads
//! `ovis.doc_profile` and `public.document`, writes only
//! `ovis.llm_annotation`, and produces text that a person reads. Nothing here
//! can stage, delete, or restore a document.
//!
//! ## Landmines
//!
//! Two rules are load-bearing enough to be asserted by tests rather than left
//! to review:
//!
//! * **Narration never writes a document-affecting table.** A source-level test
//!   greps this module for `prune_candidate`, `trash_document` and friends. The
//!   value of a read-only subsystem is entirely in it staying read-only.
//! * **Evidence is untrusted.** Every URL and title assembled here came from a
//!   crawl, so the whole block goes through [`ovis_llm::prompt`] as document
//!   text, never as instruction.

use ovis_core::db::annotation as db;
use ovis_llm::Narrator;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::services::llm as llm_service;
use crate::services::prune_triage;
use crate::state::AppState;

/// How many members of a cluster are shown to the model.
///
/// A duplicate cluster is homogeneous by construction, so the tail adds cost
/// and no information; twelve is enough to see whether the group is `?page=`
/// variants, archive editions, or genuinely distinct pages that happen to
/// hash alike.
const CLUSTER_EVIDENCE_MEMBERS: usize = 12;

/// How many sampled documents are shown for a detector bundle.
///
/// Bundles are heterogeneous — "low-quality text" spans every connector — so
/// this needs more breadth than a cluster does.
const BUNDLE_EVIDENCE_DOCS: usize = 20;

const CLUSTER_INSTRUCTION: &str = "\
You are labelling a group of documents in a search index so that a human \
reviewer can decide what to do with the group without opening every page.

The URLs below all belong to one duplicate cluster: an automated check found \
them to have identical or near-identical extracted text.

Write a `title`: a specific noun phrase naming what these pages are. Prefer \
the concrete pattern you can see (\"Print-view copies of product pages\", \
\"Archived 2019 editions of encyclopedia entries\") over a restatement of the \
obvious (\"Duplicate pages\", \"A group of similar documents\").

Write a `summary`: two sentences. The first says what the pages have in \
common. The second says what a reviewer should check before removing the \
copies — the specific risk for this group, if you can see one.

Say only what the evidence supports. If the URLs do not reveal a pattern, say \
so plainly rather than inventing one.";

const BUNDLE_INSTRUCTION: &str = "\
You are labelling a group of documents in a search index so that a human \
reviewer can decide what to do with the group without opening every page.

These documents were all flagged by the same automated detector. The \
detector's own description is given first, followed by a sample of the \
documents it flagged.

Write a `title`: a specific noun phrase naming what this sample actually \
turned out to be, which may be narrower than the detector's description. If \
the sample is dominated by one site or one kind of page, say which.

Write a `summary`: two sentences. The first says what the documents have in \
common. The second says what a reviewer should check before removing them — \
especially anything in the sample that looks like a false positive.

Say only what the evidence supports. If the sample looks mixed, say so.";

/// What a narration run is over.
///
/// An enum rather than a bare string so the accepted set, the stored
/// `subject_kind` and the instruction that goes with each cannot drift apart:
/// there is exactly one place that maps a request onto a prompt.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SubjectKind {
    Cluster,
    Bundle,
}

impl SubjectKind {
    fn parse(kind: &str) -> Option<Self> {
        match kind {
            "cluster" => Some(Self::Cluster),
            "bundle" => Some(Self::Bundle),
            _ => None,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::Cluster => "cluster",
            Self::Bundle => "bundle",
        }
    }

    fn instruction(self) -> &'static str {
        match self {
            Self::Cluster => CLUSTER_INSTRUCTION,
            Self::Bundle => BUNDLE_INSTRUCTION,
        }
    }
}

/// One narrated subject, as returned to the UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NarrationView {
    pub subject_key: String,
    pub title: String,
    pub summary: String,
    /// Shown as provenance. A generated sentence must never be mistaken for a
    /// measurement, so the surface always says which model wrote it and when.
    pub model: String,
    pub generated_at: chrono::DateTime<chrono::Utc>,
}

impl From<db::Annotation> for NarrationView {
    fn from(a: db::Annotation) -> Self {
        Self {
            subject_key: a.subject_key,
            title: a.title.unwrap_or_default(),
            summary: a.summary.unwrap_or_default(),
            model: a.model,
            generated_at: a.generated_at,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NarrateRequest {
    /// `cluster` or `bundle`.
    pub subject_kind: String,
    /// Cluster method, when narrating clusters: `hash` (default) or `url`.
    #[serde(default)]
    pub method: Option<String>,
    /// Upper bound on model calls for this run. Narration is cheap but not
    /// free, and an operator pressing a button should be able to predict what
    /// it costs.
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NarrateResponse {
    pub subject_kind: String,
    /// Subjects considered.
    pub eligible: i64,
    /// Subjects that already had this exact `(model, prompt_hash)` generation.
    pub already_current: i64,
    pub narrated: Vec<NarrationView>,
    /// Subjects that failed, with the reason. A partial run reports what it
    /// could not do rather than failing whole: one endpoint hiccup should not
    /// discard nineteen good titles.
    pub failed: Vec<FailedSubject>,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FailedSubject {
    pub subject_key: String,
    pub reason: String,
}

/// Run a narration pass.
pub async fn narrate(
    state: &AppState,
    request: NarrateRequest,
) -> Result<NarrateResponse, AppError> {
    llm_service::guard(state)?;
    let limit = request.limit.unwrap_or(25).clamp(1, 200);

    let assigned = ovis_core::db::llm::model_for_role(&state.db, "narrate")
        .await?
        .ok_or_else(|| {
            AppError::BadRequest(
                "no model is assigned to the narrate role; assign one on the Models page"
                    .into(),
            )
        })?;
    let provider_row = ovis_core::db::llm::get_provider(&state.db, assigned.provider_id)
        .await?
        .ok_or_else(|| AppError::NotFound {
            what: "llm provider",
            id: assigned.provider_id.to_string(),
        })?;
    let provider = llm_service::connect(&provider_row)?;
    let capabilities: ovis_llm::Capabilities = assigned
        .capabilities
        .clone()
        .and_then(|c| serde_json::from_value(c).ok())
        .ok_or_else(|| {
            AppError::BadRequest(format!(
                "{} has not been probed, so it cannot be trusted to return a title in the \
                 required shape; probe it on the Models page first",
                assigned.model_id
            ))
        })?;
    let narrator = Narrator::new(&provider, &assigned.model_id, capabilities)?;

    let kind = SubjectKind::parse(&request.subject_kind).ok_or_else(|| {
        AppError::BadRequest(format!(
            "unknown subject kind '{}'; expected cluster or bundle",
            request.subject_kind
        ))
    })?;
    let instruction = kind.instruction();
    let subjects = match kind {
        SubjectKind::Cluster => {
            cluster_subjects(state, request.method.as_deref(), limit).await?
        }
        SubjectKind::Bundle => bundle_subjects(state).await?,
    };

    let prompt_hash = ovis_llm::narrate::prompt_hash(instruction);
    let keys: Vec<String> = subjects.iter().map(|s| s.key.clone()).collect();
    let todo = db::missing_generation(
        &state.db,
        kind.code(),
        &keys,
        &assigned.model_id,
        &prompt_hash,
    )
    .await?;

    let mut narrated = Vec::new();
    let mut failed = Vec::new();
    for subject in subjects.iter().filter(|s| todo.contains(&s.key)).take(limit as usize) {
        match narrator.narrate(instruction, &subject.evidence).await {
            Ok(out) => {
                let stored = db::record(
                    &state.db,
                    kind.code(),
                    &subject.key,
                    &out.title,
                    &out.summary,
                    None,
                    &out.model,
                    &out.prompt_hash,
                )
                .await?;
                narrated.push(stored.into());
            }
            Err(err) => failed.push(FailedSubject {
                subject_key: subject.key.clone(),
                reason: err.to_string(),
            }),
        }
    }

    Ok(NarrateResponse {
        subject_kind: kind.code().to_string(),
        eligible: subjects.len() as i64,
        already_current: (keys.len() - todo.len()) as i64,
        narrated,
        failed,
        model: assigned.model_id,
    })
}

/// The newest annotation for each of `keys`.
pub async fn newest_for(
    state: &AppState,
    subject_kind: &str,
    keys: &[String],
) -> Result<Vec<NarrationView>, AppError> {
    // Deliberately not behind `guard`: the read path has to work when the LLM
    // tables are absent, returning nothing, so a cluster list still renders on
    // a deployment that never configured a model.
    if !state.llm_enabled {
        return Ok(Vec::new());
    }
    Ok(db::newest_for(&state.db, subject_kind, keys)
        .await?
        .into_iter()
        .map(Into::into)
        .collect())
}

// ---------------------------------------------------------------------------
// Evidence
// ---------------------------------------------------------------------------

/// A subject and the untrusted text describing it.
struct Subject {
    key: String,
    evidence: String,
}

async fn cluster_subjects(
    state: &AppState,
    method: Option<&str>,
    limit: i64,
) -> Result<Vec<Subject>, AppError> {
    let clusters = prune_triage::clusters(state, method, None, limit).await?;
    Ok(clusters
        .into_iter()
        .map(|cluster| {
            let mut evidence = format!(
                "Cluster of {} documents, grouped by {}.\n\
                 The first URL is the one the current policy would keep.\n\n",
                cluster.size,
                match cluster.method.as_str() {
                    "url" => "canonical URL after folding tracking parameters",
                    _ => "identical content hash",
                }
            );
            for member in cluster.members.iter().take(CLUSTER_EVIDENCE_MEMBERS) {
                let url = member
                    .link
                    .as_deref()
                    .or(member.semantic_id.as_deref())
                    .unwrap_or(&member.document_id);
                evidence.push_str(url);
                evidence.push('\n');
            }
            if cluster.members.len() > CLUSTER_EVIDENCE_MEMBERS {
                evidence.push_str(&format!(
                    "…and {} more with the same content.\n",
                    cluster.members.len() - CLUSTER_EVIDENCE_MEMBERS
                ));
            }
            Subject {
                key: format!("{}:{}", cluster.method, cluster.key),
                evidence,
            }
        })
        .collect())
}

async fn bundle_subjects(state: &AppState) -> Result<Vec<Subject>, AppError> {
    let overview = prune_triage::overview(state).await?;
    let mut subjects = Vec::new();
    for bundle in overview.bundles {
        // The bundle key is the reason code, which is what candidates are
        // tagged with; `detector` is the scan pass that produced it and is not
        // unique per bundle.
        let docs = sample_for_code(state, &bundle.key).await?;
        if docs.is_empty() {
            continue;
        }
        let mut evidence = format!(
            "Detector: {}\nWhat it looks for: {}\nIt flagged {} documents holding {} chunks.\n\n\
             A sample of what it flagged:\n\n",
            bundle.title, bundle.description, bundle.documents, bundle.chunks
        );
        for doc in &docs {
            evidence.push_str(doc);
            evidence.push('\n');
        }
        subjects.push(Subject {
            key: bundle.key.clone(),
            evidence,
        });
    }
    Ok(subjects)
}

/// A spread of documents carrying this reason code, as URLs.
///
/// Reason codes live in a jsonb array on the candidate, so this uses the same
/// containment match the sampling endpoint does rather than inventing a column.
///
/// Ordered by document id rather than randomly, so a re-run on an unchanged
/// corpus produces the same evidence. A title that changes on every press, with
/// nothing else having changed, reads as a bug.
async fn sample_for_code(state: &AppState, code: &str) -> Result<Vec<String>, AppError> {
    use sqlx::Row;
    let rows = sqlx::query(
        "SELECT COALESCE(d.link, d.semantic_id, c.document_id) AS url \
         FROM ovis.prune_candidate c \
         JOIN public.document d ON d.id = c.document_id \
         WHERE c.state = 'candidate' AND c.reasons @> $1 \
         ORDER BY c.document_id \
         LIMIT $2",
    )
    .bind(serde_json::json!([{ "code": code }]))
    .bind(BUNDLE_EVIDENCE_DOCS as i64)
    .fetch_all(&state.db)
    .await
    .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(rows.iter().map(|r| r.get("url")).collect())
}

#[cfg(test)]
mod tests {
    /// The reason this subsystem was built first is that it cannot move a
    /// document. That property is worth nothing if a later edit quietly adds a
    /// write, so it is asserted against the source rather than left to review.
    ///
    /// The needles are assembled at runtime so this test does not match itself.
    #[test]
    fn narration_never_writes_a_table_that_can_affect_a_document() {
        let source = include_str!("narrate.rs");
        for (a, b) in [
            ("prune_", "candidate"),
            ("trash_", "document"),
            ("pending_index_", "deletes"),
            ("doc_", "profile"),
        ] {
            let needle = format!("{a}{b}");
            for (n, line) in source.lines().enumerate() {
                let mutates = line.contains("INSERT")
                    || line.contains("UPDATE")
                    || line.contains("DELETE");
                assert!(
                    !(mutates && line.contains(&needle)),
                    "line {} writes {needle}: {line}",
                    n + 1
                );
            }
        }
    }

    /// Every kind the store accepts must be one a run can produce, and each
    /// must round-trip through its own code. Otherwise a run fails only after
    /// paying for the model call, or writes rows under a key nothing reads.
    #[test]
    fn the_accepted_subject_kinds_match_the_stores() {
        for kind in ovis_core::db::annotation::SUBJECT_KINDS {
            let parsed = super::SubjectKind::parse(kind)
                .unwrap_or_else(|| panic!("{kind} is storable but not narratable"));
            assert_eq!(parsed.code(), kind);
        }
        assert!(super::SubjectKind::parse("rule_suggestion").is_none());
    }

    /// Each kind gets its own instruction, so their annotations are versioned
    /// under different prompt hashes and cannot be confused for one another.
    #[test]
    fn each_subject_kind_has_a_distinct_prompt() {
        let cluster = super::SubjectKind::Cluster.instruction();
        let bundle = super::SubjectKind::Bundle.instruction();
        assert_ne!(
            ovis_llm::narrate::prompt_hash(cluster),
            ovis_llm::narrate::prompt_hash(bundle)
        );
    }
}
