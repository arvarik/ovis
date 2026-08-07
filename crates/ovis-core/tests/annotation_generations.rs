//! Generated titles are versioned, never overwritten.
//!
//! The rule under test is the one that makes a prompt edit safe to make: an
//! annotation is keyed by `(subject, model, prompt_hash)`, so changing either
//! the model or the prompt produces a *new generation* alongside the old one.
//! Overwriting would make it impossible to tell, after the fact, whether a
//! title changed because the corpus changed or because someone reworded an
//! instruction.

mod common;

use ovis_core::db::annotation;

const CLUSTER: &str = "hash:4f2a91";

#[tokio::test]
async fn a_changed_prompt_creates_a_generation_rather_than_overwriting() {
    let Some(db) = common::seeded().await else {
        return common::skip("a_changed_prompt_creates_a_generation_rather_than_overwriting");
    };
    assert!(annotation::ensure_tables(&db.pool).await);

    let first = annotation::record(
        &db.pool,
        "cluster",
        CLUSTER,
        "Archived encyclopedia editions",
        "Dated copies of entries that are still live at a canonical URL.",
        None,
        "gemma-4-12B",
        "aaaaaaaaaaaaaaaa",
    )
    .await
    .expect("first generation");

    // Same subject, same model, *different* prompt.
    let second = annotation::record(
        &db.pool,
        "cluster",
        CLUSTER,
        "Print-view duplicates",
        "Printer-friendly renderings of pages that also exist as HTML.",
        None,
        "gemma-4-12B",
        "bbbbbbbbbbbbbbbb",
    )
    .await
    .expect("second generation");

    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ovis.llm_annotation WHERE subject_key = $1")
            .bind(CLUSTER)
            .fetch_one(&db.pool)
            .await
            .expect("counting generations");
    assert_eq!(rows, 2, "the earlier generation must survive");

    // The read path takes the newest, so the second one is what a reviewer sees.
    let newest = annotation::newest_for(&db.pool, "cluster", &[CLUSTER.to_string()])
        .await
        .expect("newest");
    assert_eq!(newest.len(), 1);
    assert_eq!(newest[0].title.as_deref(), Some("Print-view duplicates"));
    assert!(second.generated_at >= first.generated_at);
}

/// Re-running the *same* prompt on the *same* model is a retry, not a new
/// generation — it refreshes in place rather than accumulating rows forever.
#[tokio::test]
async fn an_identical_rerun_refreshes_rather_than_accumulating() {
    let Some(db) = common::seeded().await else {
        return common::skip("an_identical_rerun_refreshes_rather_than_accumulating");
    };
    assert!(annotation::ensure_tables(&db.pool).await);

    for title in ["First attempt", "Second attempt"] {
        annotation::record(
            &db.pool, "cluster", CLUSTER, title, "Summary.", None, "m", "same",
        )
        .await
        .expect("record");
    }

    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ovis.llm_annotation WHERE subject_key = $1")
            .bind(CLUSTER)
            .fetch_one(&db.pool)
            .await
            .expect("counting");
    assert_eq!(rows, 1);

    let newest = annotation::newest_for(&db.pool, "cluster", &[CLUSTER.to_string()])
        .await
        .expect("newest");
    assert_eq!(newest[0].title.as_deref(), Some("Second attempt"));
}

/// The work list for a run is "subjects missing *this* generation", which is
/// what makes a prompt edit re-narrate everything instead of silently doing
/// nothing.
#[tokio::test]
async fn the_work_list_is_scoped_to_one_generation() {
    let Some(db) = common::seeded().await else {
        return common::skip("the_work_list_is_scoped_to_one_generation");
    };
    assert!(annotation::ensure_tables(&db.pool).await);

    let keys: Vec<String> = ["hash:a", "hash:b", "hash:c"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    annotation::record(
        &db.pool, "cluster", "hash:a", "Titled", "Summary.", None, "m", "p1",
    )
    .await
    .expect("record");

    let todo = annotation::missing_generation(&db.pool, "cluster", &keys, "m", "p1")
        .await
        .expect("work list");
    assert_eq!(todo.len(), 2, "only the untitled two are outstanding");
    assert!(!todo.contains(&"hash:a".to_string()));

    // A different prompt makes all three outstanding again.
    let todo = annotation::missing_generation(&db.pool, "cluster", &keys, "m", "p2")
        .await
        .expect("work list");
    assert_eq!(todo.len(), 3);

    // As does a different model, on the same prompt.
    let todo = annotation::missing_generation(&db.pool, "cluster", &keys, "other", "p1")
        .await
        .expect("work list");
    assert_eq!(todo.len(), 3);
}

/// A subject with no annotation is absent from the read, never present with an
/// empty title — the surface has to be able to tell "not narrated" from
/// "narrated, and it said nothing".
#[tokio::test]
async fn an_unnarrated_subject_is_absent_rather_than_blank() {
    let Some(db) = common::seeded().await else {
        return common::skip("an_unnarrated_subject_is_absent_rather_than_blank");
    };
    assert!(annotation::ensure_tables(&db.pool).await);

    let found = annotation::newest_for(&db.pool, "cluster", &["hash:nothing".to_string()])
        .await
        .expect("newest");
    assert!(found.is_empty());
}

/// A subject kind the store does not know is refused before anything is
/// written, rather than creating a row nothing will ever read.
#[tokio::test]
async fn an_unknown_subject_kind_is_refused() {
    let Some(db) = common::seeded().await else {
        return common::skip("an_unknown_subject_kind_is_refused");
    };
    assert!(annotation::ensure_tables(&db.pool).await);

    let err = annotation::record(&db.pool, "wishful", "k", "T", "S", None, "m", "p")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("unknown annotation subject kind"),
        "{err}"
    );
}
