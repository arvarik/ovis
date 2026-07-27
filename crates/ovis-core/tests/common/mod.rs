//! Shared harness for the database integration tests.
//!
//! These run against a Postgres loaded with `tests/fixtures/onyx_schema.sql` —
//! a captured `pg_dump` of the live Onyx schema — plus `seed.sql`. Bring one up
//! with:
//!
//! ```text
//! scripts/test-db.sh up
//! export OVIS_TEST_DATABASE_URL="$(scripts/test-db.sh dsn)"
//! ```
//!
//! Without `OVIS_TEST_DATABASE_URL` the tests skip themselves loudly rather than
//! failing, so `cargo test` works on a machine without Docker. The old suite did
//! the opposite: it pointed at a nonexistent database and asserted the *degraded*
//! behaviour, so it passed precisely because nothing worked.

#![allow(dead_code)]

use sqlx::PgPool;
use tokio::sync::{Mutex, MutexGuard};

/// Serialises the tests.
///
/// They share one database and each re-seeds it, so running them concurrently
/// makes them fight over the same rows. Test *isolation* here comes from taking
/// turns, not from transactions — the delete path under test commits its own
/// transaction, so there would be nothing to roll back around it.
static EXCLUSIVE: Mutex<()> = Mutex::const_new(());

/// A seeded database, held exclusively for the life of the returned value.
pub struct TestDb {
    pub pool: PgPool,
    _guard: MutexGuard<'static, ()>,
}

impl std::ops::Deref for TestDb {
    type Target = PgPool;
    fn deref(&self) -> &Self::Target {
        &self.pool
    }
}

/// A fresh pool, or `None` when no test database is configured.
///
/// One pool per test rather than a shared one: `#[tokio::test]` gives each test
/// its own runtime, and a `PgPool` is bound to the runtime that created it — a
/// shared pool starts timing out as soon as the first test's runtime shuts down.
pub async fn pool() -> Option<PgPool> {
    let dsn = std::env::var("OVIS_TEST_DATABASE_URL").ok()?;
    if dsn.trim().is_empty() {
        return None;
    }
    match ovis_core::db::create_pg_pool(&dsn, 5).await {
        Ok(pool) => Some(pool),
        Err(err) => panic!(
            "OVIS_TEST_DATABASE_URL is set but unusable: {err}. \
             Run `scripts/test-db.sh up` first."
        ),
    }
}

/// Take the database, reset it to the seed state, and hold it until the returned
/// value is dropped.
pub async fn seeded() -> Option<TestDb> {
    let guard = EXCLUSIVE.lock().await;
    let pool = pool().await?;
    reseed(&pool).await;
    Some(TestDb {
        pool,
        _guard: guard,
    })
}

pub fn skip(test: &str) {
    eprintln!(
        "SKIPPED {test}: set OVIS_TEST_DATABASE_URL (see `scripts/test-db.sh up`) \
         to run the database integration tests"
    );
}

/// Tables the seed owns, in an order that respects the foreign keys.
const SEEDED_TABLES: [&str; 13] = [
    "document__tag",
    "chunk_stats",
    "document_retrieval_feedback",
    "document_by_connector_credential_pair",
    "index_attempt_errors",
    "index_attempt",
    "background_error",
    "document",
    "tag",
    "connector_credential_pair",
    "connector",
    "credential",
    "search_settings",
];

/// Restore the seed state.
///
/// Not a transaction-per-test: the delete path under test commits its own
/// transaction, so there would be nothing left to roll back around it.
pub async fn reseed(pool: &PgPool) {
    for table in SEEDED_TABLES {
        sqlx::query(&format!("DELETE FROM public.{table}"))
            .execute(pool)
            .await
            .unwrap_or_else(|e| panic!("clearing {table}: {e}"));
    }
    sqlx::raw_sql(include_str!("../../../../tests/fixtures/seed.sql"))
        .execute(pool)
        .await
        .expect("applying tests/fixtures/seed.sql");
}

/// Document ids the seed creates, for readability at call sites.
pub mod docs {
    pub const OLDEST: &str = "https://example.com/aaa";
    pub const MIDDLE: &str = "https://example.com/bbb";
    pub const NEWEST: &str = "https://example.com/ccc";
    pub const STUB: &str = "https://example.com/stub";
    pub const UNCOUNTED: &str = "https://example.com/uncounted";
    pub const HIDDEN: &str = "https://example.com/hidden";
    pub const SHARED: &str = "https://example.com/shared";
    pub const GITHUB: &str = "https://github.com/example/thing/blob/main/README.md";
    pub const DELETE_ME: &str = "https://example.com/deleteme";
    pub const TRICKY: &str = "https://example.com/tricky?a=1&b=2 c=café";
}
