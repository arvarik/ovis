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

use sqlx::{Connection, PgConnection, PgPool};
use tokio::sync::{Mutex, MutexGuard};

/// Serialises the tests.
///
/// They share one database and each re-seeds it, so running them concurrently
/// makes them fight over the same rows. Test *isolation* here comes from taking
/// turns, not from transactions — the delete path under test commits its own
/// transaction, so there would be nothing to roll back around it.
static EXCLUSIVE: Mutex<()> = Mutex::const_new(());

/// The advisory-lock key every database-backed suite in this workspace takes
/// before reseeding.
///
/// The in-process mutex above only serialises tests *within one test binary*,
/// and `cargo test` runs binaries in parallel. Without a lock the database
/// itself arbitrates, so a suite that deletes documents (the trash round trip)
/// runs concurrently with one that counts them (the prune scans) and the
/// counts come out short. Keep this value identical in every harness.
pub const DB_LOCK_KEY: i64 = 0x0715_0000_0000_0001;

/// A seeded database, held exclusively for the life of the returned value.
pub struct TestDb {
    pub pool: PgPool,
    _lock: DbLock,
    _guard: MutexGuard<'static, ()>,
}

/// Holds a session-level advisory lock for as long as it is alive.
///
/// The lock lives on its own connection rather than one from the pool: a
/// pooled connection goes back to the pool still holding the lock, whereas
/// dropping a dedicated connection ends its session and releases it.
pub struct DbLock(Option<PgConnection>);

impl DbLock {
    pub async fn acquire(dsn: &str) -> Self {
        match PgConnection::connect(dsn).await {
            Ok(mut conn) => {
                let locked = sqlx::query("SELECT pg_advisory_lock($1)")
                    .bind(DB_LOCK_KEY)
                    .execute(&mut conn)
                    .await;
                if let Err(err) = locked {
                    // Failing open would let suites run unserialized against one
                    // database and surface as unrelated assertion failures much
                    // later. Fail loudly here instead.
                    panic!("could not take the shared test-database lock: {err}");
                }
                Self(Some(conn))
            }
            Err(err) => {
                panic!("could not open a lock connection to the test database: {err}");
            }
        }
    }
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
    let dsn = std::env::var("OVIS_TEST_DATABASE_URL").ok()?;
    if dsn.trim().is_empty() {
        return None;
    }
    // Take the cross-process lock *before* reseeding: another binary's suite
    // may be mid-scan against the rows we are about to delete.
    let lock = DbLock::acquire(&dsn).await;
    let pool = pool().await?;
    reseed(&pool).await;
    Some(TestDb {
        pool,
        _lock: lock,
        _guard: guard,
    })
}

pub fn skip(test: &str) {
    require_database_or_skip(test);
    eprintln!(
        "SKIPPED {test}: set OVIS_TEST_DATABASE_URL (see `scripts/test-db.sh up`) \
         to run the database integration tests"
    );
}

/// Refuse to skip when the caller declared a database must be present.
///
/// The DB-backed suites skip themselves when `OVIS_TEST_DATABASE_URL` is unset,
/// so `cargo test` works on a machine without Docker. That is right for a
/// laptop and dangerous for CI: cargo captures a passing test's stderr, so a
/// run with no database reports every test "ok" in 0.00 s and prints nothing at
/// all. Green, and proof of nothing.
///
/// `OVIS_REQUIRE_TEST_DATABASE=1` turns that silence into a failure. CI sets
/// it; nobody else needs to.
pub fn require_database_or_skip(test: &str) {
    if std::env::var("OVIS_REQUIRE_TEST_DATABASE")
        .map(|v| !v.trim().is_empty() && v != "0")
        .unwrap_or(false)
    {
        panic!(
            "{test} would have skipped, but OVIS_REQUIRE_TEST_DATABASE is set: \
             no usable OVIS_TEST_DATABASE_URL. A test run that skips itself here \
             proves nothing, so it fails instead."
        );
    }
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

/// OVIS-owned tables that survive a `public` reseed and would otherwise carry
/// state between tests. The trash in particular is keyed by document id, so a
/// leftover snapshot makes the next test's counts wrong in a way that looks
/// like a product bug.
const OVIS_TABLES: [&str; 12] = [
    "llm_annotation",
    "llm_role",
    "llm_model",
    "llm_provider",
    "trash_document",
    "pending_index_restores",
    "pending_index_deletes",
    "doc_profile",
    "dup_pair",
    "prune_candidate",
    "prune_audit",
    "prune_exclusions",
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
    // Best-effort: these tables only exist once the relevant `ensure_tables`
    // has run, and a fresh database legitimately has none of them.
    for table in OVIS_TABLES {
        let _ = sqlx::query(&format!("DELETE FROM ovis.{table}"))
            .execute(pool)
            .await;
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
