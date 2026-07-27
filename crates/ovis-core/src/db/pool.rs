use std::str::FromStr;
use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;

use crate::error::{CoreError, CoreResult};

/// Build the Postgres pool.
///
/// Connect **directly to Postgres**, not through pgbouncer. SQLx uses prepared
/// statements and the Onyx pgbouncer runs in transaction-pooling mode, which
/// breaks them. OVIS's pool is small enough (≤20) that it has no reason to want
/// a pooler in front of it.
///
/// `test_before_acquire` is left at its default `false`: the old setting spent an
/// extra round-trip on every single checkout to answer a question the acquire
/// timeout plus the background health heartbeat already answer.
pub async fn create_pg_pool(dsn: &str, max_connections: u32) -> CoreResult<PgPool> {
    // sqlx accepts an unknown scheme and only fails later, during DNS, with
    // "failed to lookup address information" — which sends whoever typoed the
    // scheme hunting through their network config. Check it up front.
    let scheme = dsn.split("://").next().unwrap_or_default();
    if !matches!(scheme, "postgres" | "postgresql") {
        return Err(CoreError::Invalid(format!(
            "DATABASE_URL must start with postgres:// or postgresql://, got '{scheme}://'"
        )));
    }

    let connect_opts = PgConnectOptions::from_str(dsn).map_err(|e| {
        // Never echo the DSN: it carries the password.
        CoreError::Invalid(format!(
            "DATABASE_URL is not a valid Postgres connection string: {e}"
        ))
    })?;

    let pool = PgPoolOptions::new()
        .max_connections(max_connections.max(2))
        .min_connections(2)
        .acquire_timeout(Duration::from_secs(3))
        .idle_timeout(Duration::from_secs(300))
        .max_lifetime(Duration::from_secs(1800))
        .connect_with(connect_opts)
        .await?;

    Ok(pool)
}

/// `SELECT 1` liveness probe used by the health endpoint and the background
/// heartbeat. Returns the round-trip latency.
pub async fn ping(pool: &PgPool) -> CoreResult<Duration> {
    let started = std::time::Instant::now();
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(pool)
        .await?;
    Ok(started.elapsed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_a_malformed_dsn_without_echoing_it() {
        let err = create_pg_pool("not-a-dsn://user:sekrit@host/db", 5)
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Invalid(_)));
        assert!(
            !err.to_string().contains("sekrit"),
            "the error message must not leak credentials: {err}"
        );
        assert!(
            err.to_string().contains("postgres://"),
            "the message should say what was expected: {err}"
        );
    }

    #[tokio::test]
    async fn accepts_both_postgres_schemes() {
        // Both should get past validation and fail on connection instead.
        for dsn in [
            "postgres://u:p@127.0.0.1:1/db",
            "postgresql://u:p@127.0.0.1:1/db",
        ] {
            let err = create_pg_pool(dsn, 5).await.unwrap_err();
            assert!(
                matches!(err, CoreError::Db(_)),
                "{dsn} should reach the connect attempt, got {err:?}"
            );
        }
    }
}
