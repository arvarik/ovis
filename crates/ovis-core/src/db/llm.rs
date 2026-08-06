//! Configured LLM endpoints and what each of their models was measured to do.
//!
//! Two rules shape this module.
//!
//! **Secrets never enter the database.** `api_key_ref` holds the *name* of an
//! environment variable, never its value, so a key cannot reach a backup, a
//! `pg_dump`, or a future export. The same rule the repository already applies
//! to credentials in source applies to credentials at rest.
//!
//! **Capabilities are measurements, not settings.** A row in `ovis.llm_model`
//! records what a probe observed, stamped with the probe version that observed
//! it. A model whose capabilities were recorded by an older build is visibly
//! stale rather than silently trusted — the same discipline
//! `prune_minhash.config_hash` applies to signatures.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

use crate::error::{CoreError, CoreResult};

/// What a model is used for. One model per role at a time; the roles differ in
/// cost and quality profile, so a single global "which model" setting would be
/// wrong.
pub const ROLES: [&str; 3] = ["bulk", "quality", "narrate"];

const DDL: &[&str] = &[
    "CREATE SCHEMA IF NOT EXISTS ovis",
    "CREATE TABLE IF NOT EXISTS ovis.llm_provider ( \
        id          bigserial PRIMARY KEY, \
        name        text UNIQUE NOT NULL, \
        kind        text NOT NULL, \
        base_url    text, \
        api_key_ref text, \
        enabled     boolean NOT NULL DEFAULT true, \
        created_at  timestamptz NOT NULL DEFAULT now() \
    )",
    "CREATE TABLE IF NOT EXISTS ovis.llm_model ( \
        provider_id   bigint NOT NULL REFERENCES ovis.llm_provider(id) ON DELETE CASCADE, \
        model_id      text NOT NULL, \
        display_name  text, \
        advertised    jsonb, \
        capabilities  jsonb, \
        probed_at     timestamptz, \
        probe_version int, \
        PRIMARY KEY (provider_id, model_id) \
    )",
    // Roles live in their own table rather than as a column on the model.
    // `role` as a PRIMARY KEY gives exactly one model per role, while letting
    // one model hold several — which is the common case, since `narrate`
    // usually wants whatever `quality` is. A role column on the model row made
    // those two assignments fight over the same field.
    "CREATE TABLE IF NOT EXISTS ovis.llm_role ( \
        role        text PRIMARY KEY, \
        provider_id bigint NOT NULL, \
        model_id    text NOT NULL, \
        assigned_at timestamptz NOT NULL DEFAULT now(), \
        FOREIGN KEY (provider_id, model_id) \
            REFERENCES ovis.llm_model (provider_id, model_id) ON DELETE CASCADE \
    )",
];

pub async fn ensure_tables(pool: &PgPool) -> bool {
    for statement in DDL {
        if let Err(err) = sqlx::query(statement).execute(pool).await {
            tracing::warn!(
                error = %err,
                "cannot create the ovis.llm_* tables; LLM features will report unavailable"
            );
            return false;
        }
    }
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRow {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub base_url: Option<String>,
    /// The environment variable holding the key — never the key.
    pub api_key_ref: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

fn row_to_provider(r: &sqlx::postgres::PgRow) -> ProviderRow {
    ProviderRow {
        id: r.get("id"),
        name: r.get("name"),
        kind: r.get("kind"),
        base_url: r.get("base_url"),
        api_key_ref: r.get("api_key_ref"),
        enabled: r.get("enabled"),
        created_at: r.get("created_at"),
    }
}

const PROVIDER_COLUMNS: &str =
    "SELECT id, name, kind, base_url, api_key_ref, enabled, created_at FROM ovis.llm_provider";

pub async fn list_providers(pool: &PgPool) -> CoreResult<Vec<ProviderRow>> {
    let rows = sqlx::query(&format!("{PROVIDER_COLUMNS} ORDER BY name"))
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(row_to_provider).collect())
}

pub async fn get_provider(pool: &PgPool, id: i64) -> CoreResult<Option<ProviderRow>> {
    let row = sqlx::query(&format!("{PROVIDER_COLUMNS} WHERE id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(row_to_provider))
}

pub async fn create_provider(
    pool: &PgPool,
    name: &str,
    kind: &str,
    base_url: Option<&str>,
    api_key_ref: Option<&str>,
) -> CoreResult<ProviderRow> {
    let row = sqlx::query(
        "INSERT INTO ovis.llm_provider (name, kind, base_url, api_key_ref) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, name, kind, base_url, api_key_ref, enabled, created_at",
    )
    .bind(name)
    .bind(kind)
    .bind(base_url)
    .bind(api_key_ref)
    .fetch_one(pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.constraint().is_some() => {
            CoreError::Conflict(format!("a provider named '{name}' already exists"))
        }
        _ => CoreError::Db(e),
    })?;
    Ok(row_to_provider(&row))
}

pub async fn delete_provider(pool: &PgPool, id: i64) -> CoreResult<bool> {
    let deleted = sqlx::query("DELETE FROM ovis.llm_provider WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(deleted == 1)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRow {
    pub provider_id: i64,
    pub provider_name: String,
    pub provider_kind: String,
    pub model_id: String,
    pub display_name: Option<String>,
    pub advertised: Option<serde_json::Value>,
    /// The probe result. `None` means never probed — which is different from
    /// "probed and found incapable", and the UI must not conflate them.
    pub capabilities: Option<serde_json::Value>,
    pub probed_at: Option<DateTime<Utc>>,
    pub probe_version: Option<i32>,
    /// Every role this model holds. A model may hold several.
    pub roles: Vec<String>,
}

fn row_to_model(r: &sqlx::postgres::PgRow) -> ModelRow {
    ModelRow {
        provider_id: r.get("provider_id"),
        provider_name: r.get("provider_name"),
        provider_kind: r.get("provider_kind"),
        model_id: r.get("model_id"),
        display_name: r.get("display_name"),
        advertised: r.get("advertised"),
        capabilities: r.get("capabilities"),
        probed_at: r.get("probed_at"),
        probe_version: r.get("probe_version"),
        roles: r.get::<Option<Vec<String>>, _>("roles").unwrap_or_default(),
    }
}

const MODEL_COLUMNS: &str = "\
SELECT m.provider_id, p.name AS provider_name, p.kind AS provider_kind, m.model_id, \
       m.display_name, m.advertised, m.capabilities, m.probed_at, m.probe_version, \
       COALESCE(array_agg(r.role ORDER BY r.role) FILTER (WHERE r.role IS NOT NULL), \
                ARRAY[]::text[]) AS roles \
FROM ovis.llm_model m \
JOIN ovis.llm_provider p ON p.id = m.provider_id \
LEFT JOIN ovis.llm_role r ON r.provider_id = m.provider_id AND r.model_id = m.model_id ";

/// Every non-aggregated column, for the GROUP BY the role aggregation needs.
const MODEL_GROUP_BY: &str = " GROUP BY m.provider_id, p.name, p.kind, m.model_id, \
     m.display_name, m.advertised, m.capabilities, m.probed_at, m.probe_version ";

pub async fn list_models(pool: &PgPool, provider_id: Option<i64>) -> CoreResult<Vec<ModelRow>> {
    let sql = match provider_id {
        Some(_) => format!(
            "{MODEL_COLUMNS} WHERE m.provider_id = $1 {MODEL_GROUP_BY} ORDER BY m.model_id"
        ),
        None => format!("{MODEL_COLUMNS} {MODEL_GROUP_BY} ORDER BY p.name, m.model_id"),
    };
    let mut query = sqlx::query(&sql);
    if let Some(id) = provider_id {
        query = query.bind(id);
    }
    let rows = query.fetch_all(pool).await?;
    Ok(rows.iter().map(row_to_model).collect())
}

/// Replace a provider's model list with what discovery just found.
///
/// Preserves `capabilities` and `role` for models that are still present: a
/// re-discovery is a refresh of the catalogue, not a reason to forget what was
/// measured. Models that vanished from the endpoint are dropped, which also
/// releases any role they held.
pub async fn replace_models(
    pool: &PgPool,
    provider_id: i64,
    models: &[(String, Option<String>, serde_json::Value)],
) -> CoreResult<u64> {
    let mut tx = pool.begin().await?;

    let ids: Vec<String> = models.iter().map(|(id, _, _)| id.clone()).collect();
    sqlx::query("DELETE FROM ovis.llm_model WHERE provider_id = $1 AND model_id <> ALL($2)")
        .bind(provider_id)
        .bind(&ids)
        .execute(&mut *tx)
        .await?;

    let mut written = 0u64;
    for (model_id, display_name, advertised) in models {
        written += sqlx::query(
            "INSERT INTO ovis.llm_model (provider_id, model_id, display_name, advertised) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (provider_id, model_id) DO UPDATE \
               SET display_name = excluded.display_name, advertised = excluded.advertised",
        )
        .bind(provider_id)
        .bind(model_id)
        .bind(display_name)
        .bind(advertised)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    }
    tx.commit().await?;
    Ok(written)
}

pub async fn record_capabilities(
    pool: &PgPool,
    provider_id: i64,
    model_id: &str,
    capabilities: &serde_json::Value,
    probe_version: i32,
) -> CoreResult<bool> {
    let updated = sqlx::query(
        "UPDATE ovis.llm_model \
         SET capabilities = $3, probed_at = now(), probe_version = $4 \
         WHERE provider_id = $1 AND model_id = $2",
    )
    .bind(provider_id)
    .bind(model_id)
    .bind(capabilities)
    .bind(probe_version)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// Assign a role, releasing whoever held it.
///
/// Refuses a model that has not been probed, or one whose probe found no
/// enforced constraint — the enforcement point for "an unconstrained model
/// never judges", applied at the database boundary as well as in `Judge::new`.
pub async fn assign_role(
    pool: &PgPool,
    role: &str,
    provider_id: i64,
    model_id: &str,
) -> CoreResult<()> {
    if !ROLES.contains(&role) {
        return Err(CoreError::Invalid(format!(
            "unknown role '{role}'; expected one of {}",
            ROLES.join(", ")
        )));
    }

    let capabilities: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT capabilities FROM ovis.llm_model WHERE provider_id = $1 AND model_id = $2",
    )
    .bind(provider_id)
    .bind(model_id)
    .fetch_optional(pool)
    .await?
    .flatten();

    let usable = capabilities
        .as_ref()
        .map(|c| {
            c.get("enum_enforced").and_then(serde_json::Value::as_bool) == Some(true)
                || c.get("schema_enforced").and_then(serde_json::Value::as_bool) == Some(true)
        })
        .unwrap_or(false);
    if !usable {
        return Err(CoreError::Invalid(format!(
            "{model_id} has not been probed, or its probe found no enforced output constraint. \
             Probe it first; a model that cannot be constrained must not be given a role, \
             because a document could make it emit arbitrary text."
        )));
    }

    sqlx::query(
        "INSERT INTO ovis.llm_role (role, provider_id, model_id) VALUES ($1, $2, $3) \
         ON CONFLICT (role) DO UPDATE \
           SET provider_id = excluded.provider_id, model_id = excluded.model_id, \
               assigned_at = now()",
    )
    .bind(role)
    .bind(provider_id)
    .bind(model_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn clear_role(pool: &PgPool, role: &str) -> CoreResult<()> {
    sqlx::query("DELETE FROM ovis.llm_role WHERE role = $1")
        .bind(role)
        .execute(pool)
        .await?;
    Ok(())
}

/// The model currently holding a role, if any.
pub async fn model_for_role(pool: &PgPool, role: &str) -> CoreResult<Option<ModelRow>> {
    let row = sqlx::query(&format!(
        "{MODEL_COLUMNS} WHERE EXISTS (SELECT 1 FROM ovis.llm_role x \
         WHERE x.role = $1 AND x.provider_id = m.provider_id AND x.model_id = m.model_id) \
         {MODEL_GROUP_BY}"
    ))
    .bind(role)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_model))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_is_confined_to_the_ovis_schema() {
        for statement in DDL {
            let lowered = statement.to_lowercase();
            if lowered.starts_with("create table") {
                assert!(lowered.contains("ovis."), "{statement}");
            }
            if lowered.contains("create unique index") {
                assert!(lowered.contains(" on ovis."), "{statement}");
            }
        }
    }

    #[test]
    fn no_write_statement_targets_an_onyx_table() {
        let source = include_str!("llm.rs");
        for (idx, line) in source.lines().enumerate() {
            let lowered = line.to_lowercase();
            for verb in ["insert into ", "update ", "delete from "] {
                if let Some(pos) = lowered.find(verb) {
                    let target = lowered[pos + verb.len()..]
                        .split_whitespace()
                        .next()
                        .unwrap_or("");
                    assert!(
                        !target.starts_with("public."),
                        "line {}: LLM config writes an Onyx table: {}",
                        idx + 1,
                        line.trim()
                    );
                }
            }
        }
    }

    /// The whole point of `api_key_ref`: a key value must never be bound into
    /// a statement. Asserted structurally so the column cannot quietly change
    /// meaning later.
    ///
    /// Needles are assembled at runtime so this test cannot match its own
    /// source — the same trap the reaper's landmine tests avoid.
    #[test]
    fn only_a_key_reference_is_ever_persisted() {
        let source = include_str!("llm.rs");
        let column = format!("api_key{}", " ");
        assert!(
            !source.contains(&column),
            "the column is api_key_ref — a variable name, never a value"
        );
        for expr in ["api_key", "&api_key", "key"] {
            let forbidden = format!("bind({expr})");
            assert!(
                !source.contains(&forbidden),
                "`{forbidden}` would put a secret in the database"
            );
        }
    }

    #[test]
    fn roles_are_the_three_documented_ones() {
        assert_eq!(ROLES, ["bulk", "quality", "narrate"]);
    }

    /// Exactly one model per role, but a model may hold several — `narrate`
    /// usually wants whatever `quality` is, and an earlier schema that put
    /// `role` on the model row made those two assignments overwrite each other.
    #[test]
    fn one_model_per_role_but_a_model_may_hold_several() {
        let ddl = DDL.join(" ");
        assert!(ddl.contains("ovis.llm_role"));
        assert!(
            ddl.contains("role        text PRIMARY KEY"),
            "role must be the primary key so a role has one holder"
        );
        assert!(
            !ddl.contains("role          text,"),
            "role must not be a column on the model row"
        );
    }
}
