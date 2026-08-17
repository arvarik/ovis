//! The trash: deletion that Onyx cannot see and OVIS can undo.
//!
//! Before v2 the reaper's cascade was the end of a document. Postgres rows
//! gone, chunks gone, no record of the content anywhere — so however careful
//! the staging lifecycle was, the last step was still irreversible, and that
//! ceiling is what kept anyone from pruning aggressively.
//!
//! Here the cascade is preceded by a **snapshot**: the `public.document` row,
//! its tags, its connector attribution, and every chunk's verbatim
//! OpenSearch `_source` — embedding vectors included. The snapshot and the
//! deletion share one transaction, so the two possible outcomes are "document
//! present, no snapshot" and "document gone, snapshot exists". There is no
//! third state where content is lost.
//!
//! Onyx invisibility is structural rather than promised: the document row is
//! genuinely deleted and the chunks are genuinely removed from the index, so
//! nothing in Onyx — search, connectors, the admin UI — has anything left to
//! find. The bytes live in the `ovis` schema, which Onyx never reads.
//!
//! Restore re-inserts the Postgres rows and bulk-indexes the chunks back under
//! their original `_id`s. Because the vectors came along, a restored document
//! answers semantic queries immediately; nothing waits on a re-crawl or a
//! re-embed.

use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Acquire, PgPool, Row};

use crate::error::{CoreError, CoreResult};
use crate::search::OsClient;

/// Bumped whenever the snapshot layout changes in a way an older restore
/// could not read. A snapshot from the future is refused loudly rather than
/// half-restored.
pub const SNAPSHOT_VERSION: i32 = 1;

const DDL: &[&str] = &[
    "CREATE SCHEMA IF NOT EXISTS ovis",
    // `snapshot` is jsonb rather than a compressed blob on purpose: Postgres
    // TOASTs and compresses it anyway, and keeping it queryable means a
    // half-broken restore can be diagnosed in SQL instead of by writing a
    // decoder.
    "CREATE TABLE IF NOT EXISTS ovis.trash_document ( \
        document_id      text PRIMARY KEY, \
        snapshot         jsonb NOT NULL, \
        snapshot_version int NOT NULL DEFAULT 1, \
        snapshot_bytes   bigint NOT NULL DEFAULT 0, \
        chunk_count      int NOT NULL DEFAULT 0, \
        vectors_included boolean NOT NULL DEFAULT false, \
        semantic_id      text, \
        connector_id     int, \
        candidate_id     bigint, \
        policy_hash      text, \
        reasons          jsonb, \
        deleted_by       text NOT NULL DEFAULT 'unknown', \
        deleted_at       timestamptz NOT NULL DEFAULT now(), \
        expires_at       timestamptz NOT NULL, \
        hold             boolean NOT NULL DEFAULT false, \
        restored_at      timestamptz \
    )",
    "CREATE INDEX IF NOT EXISTS ix_ovis_trash_expiry \
        ON ovis.trash_document (hold, expires_at) WHERE restored_at IS NULL",
    "CREATE INDEX IF NOT EXISTS ix_ovis_trash_deleted_at \
        ON ovis.trash_document (deleted_at DESC)",
    "CREATE INDEX IF NOT EXISTS ix_ovis_trash_connector \
        ON ovis.trash_document (connector_id)",
    // Chunks that failed to come back during a restore, retried by a drain
    // task — the mirror of pending_index_deletes.
    "CREATE TABLE IF NOT EXISTS ovis.pending_index_restores ( \
        document_id text PRIMARY KEY, \
        payload     jsonb NOT NULL, \
        queued_at   timestamptz NOT NULL DEFAULT now(), \
        attempts    int NOT NULL DEFAULT 0, \
        last_attempt timestamptz, \
        last_error  text \
    )",
];

pub async fn ensure_tables(pool: &PgPool) -> bool {
    for statement in DDL {
        if let Err(err) = sqlx::query(*statement).execute(pool).await {
            tracing::warn!(
                error = %err,
                "cannot create ovis.trash_document; deletion will refuse to run rather than \
                 delete irreversibly"
            );
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

/// Everything needed to bring one document back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: i32,
    pub document_id: String,
    /// The full `public.document` row as column → value.
    pub document: Value,
    /// `(tag_id, tag_key, tag_value)` for each tag link.
    pub tags: Vec<Value>,
    /// `document_by_connector_credential_pair` rows.
    pub cc_pairs: Vec<Value>,
    /// `(_id, _source)` per chunk, verbatim, vectors included when captured.
    pub chunks: Vec<Value>,
    pub vectors_included: bool,
    pub captured_at: DateTime<Utc>,
}

impl Snapshot {
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Approximate serialized size, reported to the UI so "how much is in the
    /// trash" is a real number rather than a row count.
    pub fn byte_size(&self) -> i64 {
        serde_json::to_vec(self)
            .map(|v| v.len() as i64)
            .unwrap_or(0)
    }

    pub fn semantic_id(&self) -> Option<String> {
        self.document
            .get("semantic_id")
            .and_then(Value::as_str)
            .map(String::from)
    }

    pub fn connector_id(&self) -> Option<i32> {
        self.cc_pairs
            .first()
            .and_then(|p| p.get("connector_id"))
            .and_then(Value::as_i64)
            .map(|v| v as i32)
    }
}

/// Read every part of a document that deletion would destroy.
///
/// Runs entirely **before** the delete transaction: an OpenSearch that cannot
/// answer must stop the deletion, not produce a snapshot with no chunks in it.
/// A document with genuinely zero chunks (the stub case) snapshots fine — the
/// distinction is between "asked and got nothing" and "could not ask".
pub async fn capture(
    pool: &PgPool,
    os: &OsClient,
    index: &str,
    document_id: &str,
    keep_vectors: bool,
) -> CoreResult<Snapshot> {
    let row = sqlx::query("SELECT * FROM public.document WHERE id = $1")
        .bind(document_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| CoreError::not_found("document", document_id))?;
    let document = row_to_json(&row);

    let tag_rows = sqlx::query(
        "SELECT dt.tag_id, t.tag_key, t.tag_value, t.source \
         FROM public.document__tag dt JOIN public.tag t ON t.id = dt.tag_id \
         WHERE dt.document_id = $1",
    )
    .bind(document_id)
    .fetch_all(pool)
    .await?;
    let tags: Vec<Value> = tag_rows.iter().map(row_to_json).collect();

    let cc_rows =
        sqlx::query("SELECT * FROM public.document_by_connector_credential_pair WHERE id = $1")
            .bind(document_id)
            .fetch_all(pool)
            .await?;
    let cc_pairs: Vec<Value> = cc_rows.iter().map(row_to_json).collect();

    let mut chunks: Vec<Value> = Vec::new();
    let mut after: Option<i64> = None;
    let mut expected: Option<i64> = None;
    loop {
        let (items, total, next) = os
            .document_chunks_raw(index, document_id, after, 200)
            .await?;
        expected.get_or_insert(total);
        if items.is_empty() {
            break;
        }
        for (id, mut source) in items {
            if !keep_vectors {
                if let Some(obj) = source.as_object_mut() {
                    obj.remove("content_vector");
                    obj.remove("title_vector");
                }
            } else {
                compact_vectors(&mut source);
            }
            chunks.push(json!({ "_id": id, "_source": source }));
        }
        match next {
            Some(n) => after = Some(n),
            None => break,
        }
    }

    // Paging stops on the first page that cannot produce a cursor, which is
    // normally the last one — but a chunk missing the sort field ends it early
    // too, and a snapshot short of a chunk looks exactly like a complete one.
    // Deletion is irreversible past the trash, so a short read is refused
    // rather than trusted. Losing a delete cycle is recoverable; losing a
    // chunk is not.
    let expected = expected.unwrap_or(0);
    if (chunks.len() as i64) < expected {
        return Err(CoreError::search(format!(
            "snapshot for {document_id} read {} of {expected} chunks; refusing to delete a \
             document it cannot fully capture",
            chunks.len()
        )));
    }

    Ok(Snapshot {
        version: SNAPSHOT_VERSION,
        document_id: document_id.to_string(),
        document,
        tags,
        cc_pairs,
        chunks,
        vectors_included: keep_vectors,
        captured_at: Utc::now(),
    })
}

/// Vector fields are stored as base64 of little-endian `f16` rather than JSON
/// float arrays: 768 floats render to roughly 7.6 kB of text each, against
/// 2 kB packed, and the extra precision is meaningless for a vector that will
/// be re-inserted and compared by cosine.
fn compact_vectors(source: &mut Value) {
    let Some(obj) = source.as_object_mut() else {
        return;
    };
    for field in ["content_vector", "title_vector"] {
        let Some(Value::Array(values)) = obj.get(field) else {
            continue;
        };
        let mut bytes = Vec::with_capacity(values.len() * 2);
        let mut ok = true;
        for value in values {
            match value.as_f64() {
                Some(v) => bytes.extend_from_slice(&f32_to_f16_bits(v as f32).to_le_bytes()),
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
            obj.insert(
                field.to_string(),
                json!({ "__f16_b64": encoded, "dim": values.len() }),
            );
        }
    }
}

fn expand_vectors(source: &mut Value) {
    let Some(obj) = source.as_object_mut() else {
        return;
    };
    for field in ["content_vector", "title_vector"] {
        let Some(packed) = obj
            .get(field)
            .and_then(|v| v.get("__f16_b64"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(packed) else {
            continue;
        };
        let floats: Vec<Value> = bytes
            .chunks_exact(2)
            .map(|pair| {
                let bits = u16::from_le_bytes([pair[0], pair[1]]);
                json!(f16_bits_to_f32(bits))
            })
            .collect();
        obj.insert(field.to_string(), Value::Array(floats));
    }
}

/// Round-to-nearest-even `f32` → `f16` bit pattern, with saturation.
fn f32_to_f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x007f_ffff;

    if exponent == 0xff {
        // Inf or NaN: keep the class, collapse the payload.
        return sign | 0x7c00 | if mantissa != 0 { 0x0200 } else { 0 };
    }
    let unbiased = exponent - 127 + 15;
    if unbiased >= 0x1f {
        return sign | 0x7c00; // overflow → infinity
    }
    if unbiased <= 0 {
        if unbiased < -10 {
            return sign; // underflow → zero
        }
        // Subnormal: shift the implicit leading bit back in.
        let mantissa = mantissa | 0x0080_0000;
        let shift = (14 - unbiased) as u32;
        let half = (mantissa >> shift) as u16;
        let round = u16::from((mantissa >> (shift - 1)) & 1 == 1);
        return sign | (half + round);
    }
    let half = sign | ((unbiased as u16) << 10) | ((mantissa >> 13) as u16);
    // Round to nearest, ties to even.
    let round = u16::from(
        (mantissa & 0x1fff) > 0x1000 || ((mantissa & 0x1fff) == 0x1000 && (half & 1) == 1),
    );
    half + round
}

fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign = ((bits & 0x8000) as u32) << 16;
    let exponent = ((bits >> 10) & 0x1f) as u32;
    let mantissa = (bits & 0x03ff) as u32;
    let out = match exponent {
        0 if mantissa == 0 => sign,
        0 => {
            // Subnormal: renormalize.
            let mut exp = 127 - 15 + 1;
            let mut man = mantissa;
            while man & 0x0400 == 0 {
                man <<= 1;
                exp -= 1;
            }
            sign | (exp << 23) | ((man & 0x03ff) << 13)
        }
        0x1f => sign | 0x7f80_0000 | (mantissa << 13),
        _ => sign | ((exponent + 127 - 15) << 23) | (mantissa << 13),
    };
    f32::from_bits(out)
}

fn row_to_json(row: &sqlx::postgres::PgRow) -> Value {
    use sqlx::{Column, TypeInfo, ValueRef};
    let mut map = serde_json::Map::new();
    for (idx, column) in row.columns().iter().enumerate() {
        let raw = match row.try_get_raw(idx) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        if raw.is_null() {
            map.insert(column.name().to_string(), Value::Null);
            continue;
        }
        // Decode by declared type: the snapshot has to survive a round trip
        // through JSON and back into the same column types.
        let value = match column.type_info().name() {
            "INT2" => row
                .try_get::<i16, _>(idx)
                .map(|v| json!(v))
                .unwrap_or(Value::Null),
            "INT4" => row
                .try_get::<i32, _>(idx)
                .map(|v| json!(v))
                .unwrap_or(Value::Null),
            "INT8" => row
                .try_get::<i64, _>(idx)
                .map(|v| json!(v))
                .unwrap_or(Value::Null),
            "FLOAT4" => row
                .try_get::<f32, _>(idx)
                .map(|v| json!(v))
                .unwrap_or(Value::Null),
            "FLOAT8" => row
                .try_get::<f64, _>(idx)
                .map(|v| json!(v))
                .unwrap_or(Value::Null),
            "BOOL" => row
                .try_get::<bool, _>(idx)
                .map(|v| json!(v))
                .unwrap_or(Value::Null),
            "TIMESTAMPTZ" => row
                .try_get::<DateTime<Utc>, _>(idx)
                .map(|v| json!(v.to_rfc3339()))
                .unwrap_or(Value::Null),
            "JSON" | "JSONB" => row.try_get::<Value, _>(idx).unwrap_or(Value::Null),
            "TEXT[]" | "VARCHAR[]" => row
                .try_get::<Vec<String>, _>(idx)
                .map(|v| json!(v))
                .unwrap_or(Value::Null),
            _ => row
                .try_get::<String, _>(idx)
                .map(|v| json!(v))
                .unwrap_or(Value::Null),
        };
        map.insert(column.name().to_string(), value);
    }
    Value::Object(map)
}

// ---------------------------------------------------------------------------
// Trash (capture + delete, atomically)
// ---------------------------------------------------------------------------

/// Provenance recorded alongside a snapshot, so the Trash tab can say *why*
/// something was deleted and under which policy.
#[derive(Debug, Clone, Default)]
pub struct TrashProvenance {
    pub candidate_id: Option<i64>,
    pub policy_hash: Option<String>,
    pub reasons: Option<Value>,
    pub deleted_by: String,
}

/// Write the snapshot and run the FK-complete cascade in **one** transaction.
///
/// Either both happen or neither does. On success the document no longer
/// exists anywhere in Onyx's Postgres; the caller still has to remove its
/// chunks from the index, and a failure there is ordinary index-cleanup debt
/// rather than data loss — the chunk bodies are inside the snapshot.
pub async fn trash_and_delete(
    pool: &PgPool,
    snapshot: &Snapshot,
    provenance: &TrashProvenance,
    retention_days: i64,
) -> CoreResult<i64> {
    if snapshot.version > SNAPSHOT_VERSION {
        return Err(CoreError::Invalid(format!(
            "snapshot version {} is newer than this build understands ({SNAPSHOT_VERSION})",
            snapshot.version
        )));
    }
    let payload = serde_json::to_value(snapshot)
        .map_err(|e| CoreError::Invalid(format!("unserialisable snapshot: {e}")))?;
    let bytes = snapshot.byte_size();

    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO ovis.trash_document \
             (document_id, snapshot, snapshot_version, snapshot_bytes, chunk_count, \
              vectors_included, semantic_id, connector_id, candidate_id, policy_hash, \
              reasons, deleted_by, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, \
                 now() + make_interval(days => $13)) \
         ON CONFLICT (document_id) DO UPDATE SET \
             snapshot = excluded.snapshot, snapshot_version = excluded.snapshot_version, \
             snapshot_bytes = excluded.snapshot_bytes, chunk_count = excluded.chunk_count, \
             vectors_included = excluded.vectors_included, \
             semantic_id = excluded.semantic_id, connector_id = excluded.connector_id, \
             candidate_id = excluded.candidate_id, policy_hash = excluded.policy_hash, \
             reasons = excluded.reasons, deleted_by = excluded.deleted_by, \
             deleted_at = now(), expires_at = excluded.expires_at, restored_at = NULL",
    )
    .bind(&snapshot.document_id)
    .bind(&payload)
    .bind(snapshot.version)
    .bind(bytes)
    .bind(snapshot.chunk_count() as i32)
    .bind(snapshot.vectors_included)
    .bind(snapshot.semantic_id())
    .bind(snapshot.connector_id())
    .bind(provenance.candidate_id)
    .bind(&provenance.policy_hash)
    .bind(&provenance.reasons)
    .bind(&provenance.deleted_by)
    .bind(retention_days.clamp(1, 365) as i32)
    .execute(&mut *tx)
    .await?;

    super::documents::delete_document_in_tx(&mut tx, &snapshot.document_id).await?;
    tx.commit().await?;
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Restore
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreOutcome {
    pub document_id: String,
    pub chunks_restored: u64,
    pub tags_restored: usize,
    pub cc_pairs_restored: usize,
    /// Tag links whose `tag` row no longer exists, and connector attributions
    /// whose connector is gone. Reported rather than silently dropped.
    pub skipped_tags: usize,
    pub skipped_cc_pairs: usize,
    pub index_restore_pending: bool,
}

/// Bring one document back.
///
/// Refuses rather than guesses when the id already exists in Onyx (a recrawl
/// beat the restore); the caller decides between discarding the snapshot and
/// overwriting. Tag and connector rows whose parents have since been deleted
/// are skipped and counted — a restore that silently dropped half a
/// document's attribution would be worse than one that says so.
pub async fn restore(
    pool: &PgPool,
    os: &OsClient,
    index: &str,
    document_id: &str,
    overwrite: bool,
) -> CoreResult<RestoreOutcome> {
    let row = sqlx::query(
        "SELECT snapshot, snapshot_version FROM ovis.trash_document \
         WHERE document_id = $1 AND restored_at IS NULL",
    )
    .bind(document_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| CoreError::not_found("trashed document", document_id))?;

    let version: i32 = row.get("snapshot_version");
    if version > SNAPSHOT_VERSION {
        return Err(CoreError::Invalid(format!(
            "snapshot for {document_id} is version {version}, newer than this build \
             understands ({SNAPSHOT_VERSION}); refusing a partial restore"
        )));
    }
    let snapshot: Snapshot =
        serde_json::from_value(row.get::<Value, _>("snapshot")).map_err(|e| {
            CoreError::Invalid(format!("snapshot for {document_id} is unreadable: {e}"))
        })?;

    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM public.document WHERE id = $1)")
            .bind(document_id)
            .fetch_one(pool)
            .await?;
    if exists && !overwrite {
        return Err(CoreError::Conflict(format!(
            "{document_id} already exists in Onyx — the crawler brought it back. Restore with \
             overwrite to replace it, or discard the snapshot."
        )));
    }

    let mut tx = pool.begin().await?;
    if exists {
        super::documents::delete_document_in_tx(&mut tx, document_id).await?;
    }

    // `jsonb_populate_record` rebuilds the row against the *live* table
    // definition, so Postgres performs every type coercion itself — timestamps,
    // arrays, jsonb, enums. Reconstructing the INSERT column by column meant
    // re-implementing that coercion in Rust, and getting `timestamptz` wrong
    // was enough to fail a restore outright.
    if !snapshot.document.is_object() {
        return Err(CoreError::Invalid(format!(
            "snapshot for {document_id} has no document row"
        )));
    }
    sqlx::query(
        "INSERT INTO public.document SELECT * FROM \
         jsonb_populate_record(null::public.document, $1)",
    )
    .bind(&snapshot.document)
    .execute(&mut *tx)
    .await?;

    let mut tags_restored = 0usize;
    let mut skipped_tags = 0usize;
    for tag in &snapshot.tags {
        let Some(tag_id) = tag.get("tag_id").and_then(Value::as_i64) else {
            skipped_tags += 1;
            continue;
        };
        let affected = sqlx::query(
            "INSERT INTO public.document__tag (document_id, tag_id) \
             SELECT $1, $2 WHERE EXISTS (SELECT 1 FROM public.tag WHERE id = $2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(document_id)
        .bind(tag_id as i32)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if affected == 1 {
            tags_restored += 1;
        } else {
            skipped_tags += 1;
        }
    }

    let mut cc_pairs_restored = 0usize;
    let mut skipped_cc_pairs = 0usize;
    for pair in &snapshot.cc_pairs {
        // A vanished connector is a foreign-key error, not a reason to lose
        // the document — so each attribution row gets its own savepoint and a
        // failure skips just that row.
        let mut savepoint = tx.begin().await?;
        let inserted = sqlx::query(
            "INSERT INTO public.document_by_connector_credential_pair \
             SELECT * FROM jsonb_populate_record( \
                 null::public.document_by_connector_credential_pair, $1) \
             ON CONFLICT DO NOTHING",
        )
        .bind(pair)
        .execute(&mut *savepoint)
        .await;
        match inserted {
            Ok(result) if result.rows_affected() == 1 => {
                savepoint.commit().await?;
                cc_pairs_restored += 1;
            }
            Ok(_) => {
                savepoint.commit().await?;
                skipped_cc_pairs += 1;
            }
            Err(err) => {
                tracing::debug!(document_id, error = %err, "connector attribution not restorable");
                savepoint.rollback().await?;
                skipped_cc_pairs += 1;
            }
        }
    }
    tx.commit().await?;

    // Postgres is committed; the index is best-effort with a retry queue, the
    // same shape the delete path already uses.
    let payload: Vec<(String, Value)> = snapshot
        .chunks
        .iter()
        .filter_map(|chunk| {
            let id = chunk.get("_id")?.as_str()?.to_string();
            let mut source = chunk.get("_source")?.clone();
            expand_vectors(&mut source);
            Some((id, source))
        })
        .collect();

    let (chunks_restored, index_restore_pending) = match os.bulk_index_chunks(index, &payload).await
    {
        Ok(n) => (n, false),
        Err(err) => {
            tracing::warn!(
                document_id = %document_id,
                error = %err,
                "document rows restored but chunk re-indexing failed; queued for retry"
            );
            let _ = enqueue_index_restore(pool, document_id, &payload, &err.to_string()).await;
            (0, true)
        }
    };

    sqlx::query("UPDATE ovis.trash_document SET restored_at = now() WHERE document_id = $1")
        .bind(document_id)
        .execute(pool)
        .await?;

    Ok(RestoreOutcome {
        document_id: document_id.to_string(),
        chunks_restored,
        tags_restored,
        cc_pairs_restored,
        skipped_tags,
        skipped_cc_pairs,
        index_restore_pending,
    })
}

async fn enqueue_index_restore(
    pool: &PgPool,
    document_id: &str,
    payload: &[(String, Value)],
    error: &str,
) -> CoreResult<()> {
    let payload = json!(payload
        .iter()
        .map(|(id, source)| json!({ "_id": id, "_source": source }))
        .collect::<Vec<_>>());
    sqlx::query(
        "INSERT INTO ovis.pending_index_restores (document_id, payload, last_error, attempts) \
         VALUES ($1, $2, $3, 1) \
         ON CONFLICT (document_id) DO UPDATE SET payload = excluded.payload, \
             last_error = excluded.last_error, attempts = ovis.pending_index_restores.attempts + 1, \
             last_attempt = now()",
    )
    .bind(document_id)
    .bind(payload)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Listing, holds, purge
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrashItem {
    pub document_id: String,
    pub semantic_id: Option<String>,
    pub connector_id: Option<i32>,
    pub connector_name: Option<String>,
    pub chunk_count: i32,
    pub snapshot_bytes: i64,
    pub vectors_included: bool,
    pub reasons: Option<Value>,
    pub policy_hash: Option<String>,
    pub deleted_by: String,
    pub deleted_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub hold: bool,
    /// True when the id exists in Onyx again — restoring would collide.
    pub reappeared: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TrashFilter {
    pub connector_id: Option<i32>,
    pub document_id: Option<String>,
    pub hold: Option<bool>,
    /// Only items whose retention ends within this many days.
    pub expiring_within_days: Option<i64>,
}

pub async fn list(
    pool: &PgPool,
    filter: &TrashFilter,
    limit: i64,
    offset: i64,
) -> CoreResult<(Vec<TrashItem>, i64)> {
    let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
        "SELECT t.document_id, t.semantic_id, t.connector_id, c.name AS connector_name, \
                t.chunk_count, t.snapshot_bytes, t.vectors_included, t.reasons, t.policy_hash, \
                t.deleted_by, t.deleted_at, t.expires_at, t.hold, \
                EXISTS (SELECT 1 FROM public.document d WHERE d.id = t.document_id) AS reappeared, \
                count(*) OVER () AS total \
         FROM ovis.trash_document t \
         LEFT JOIN public.connector c ON c.id = t.connector_id \
         WHERE t.restored_at IS NULL",
    );
    if let Some(connector_id) = filter.connector_id {
        qb.push(" AND t.connector_id = ").push_bind(connector_id);
    }
    if let Some(document_id) = &filter.document_id {
        qb.push(" AND t.document_id = ")
            .push_bind(document_id.clone());
    }
    if let Some(hold) = filter.hold {
        qb.push(" AND t.hold = ").push_bind(hold);
    }
    if let Some(days) = filter.expiring_within_days {
        qb.push(" AND t.expires_at <= now() + make_interval(days => ")
            .push_bind(days as i32)
            .push(")");
    }
    qb.push(" ORDER BY t.deleted_at DESC, t.document_id LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);

    let rows = qb.build().fetch_all(pool).await?;
    let total = rows.first().map(|r| r.get::<i64, _>("total")).unwrap_or(0);
    Ok((
        rows.iter()
            .map(|r| TrashItem {
                document_id: r.get("document_id"),
                semantic_id: r.get("semantic_id"),
                connector_id: r.get("connector_id"),
                connector_name: r.get("connector_name"),
                chunk_count: r.get("chunk_count"),
                snapshot_bytes: r.get("snapshot_bytes"),
                vectors_included: r.get("vectors_included"),
                reasons: r.get("reasons"),
                policy_hash: r.get("policy_hash"),
                deleted_by: r.get("deleted_by"),
                deleted_at: r.get("deleted_at"),
                expires_at: r.get("expires_at"),
                hold: r.get("hold"),
                reappeared: r.get("reappeared"),
            })
            .collect(),
        total,
    ))
}

/// The stored snapshot, for the inspector and for `download`.
pub async fn get_snapshot(pool: &PgPool, document_id: &str) -> CoreResult<Option<Snapshot>> {
    let row = sqlx::query("SELECT snapshot FROM ovis.trash_document WHERE document_id = $1")
        .bind(document_id)
        .fetch_optional(pool)
        .await?;
    match row {
        None => Ok(None),
        Some(row) => {
            let snapshot: Snapshot = serde_json::from_value(row.get::<Value, _>("snapshot"))
                .map_err(|e| CoreError::Invalid(format!("snapshot is unreadable: {e}")))?;
            Ok(Some(snapshot))
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TrashCounts {
    pub items: i64,
    pub bytes: i64,
    pub expiring_7d: i64,
    pub on_hold: i64,
    pub restored_total: i64,
    pub soonest_expiry: Option<DateTime<Utc>>,
}

pub async fn counts(pool: &PgPool) -> CoreResult<TrashCounts> {
    let row = sqlx::query(
        "SELECT count(*) FILTER (WHERE restored_at IS NULL) AS items, \
                COALESCE(sum(snapshot_bytes) FILTER (WHERE restored_at IS NULL), 0)::bigint AS bytes, \
                count(*) FILTER (WHERE restored_at IS NULL AND NOT hold \
                                 AND expires_at < now() + interval '7 days') AS expiring_7d, \
                count(*) FILTER (WHERE restored_at IS NULL AND hold) AS on_hold, \
                count(*) FILTER (WHERE restored_at IS NOT NULL) AS restored_total, \
                min(expires_at) FILTER (WHERE restored_at IS NULL AND NOT hold) AS soonest \
         FROM ovis.trash_document",
    )
    .fetch_one(pool)
    .await?;
    Ok(TrashCounts {
        items: row.get("items"),
        bytes: row.get::<i64, _>("bytes"),
        expiring_7d: row.get("expiring_7d"),
        on_hold: row.get("on_hold"),
        restored_total: row.get("restored_total"),
        soonest_expiry: row.get("soonest"),
    })
}

pub async fn set_hold(pool: &PgPool, document_id: &str, hold: bool) -> CoreResult<bool> {
    let updated = sqlx::query(
        "UPDATE ovis.trash_document SET hold = $2 WHERE document_id = $1 AND restored_at IS NULL",
    )
    .bind(document_id)
    .bind(hold)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// Snapshots whose retention has run out, oldest first. Held items are never
/// returned, at any age.
pub async fn due_for_purge(pool: &PgPool, limit: i64) -> CoreResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT document_id FROM ovis.trash_document \
         WHERE restored_at IS NULL AND NOT hold AND expires_at <= now() \
         ORDER BY expires_at LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Drop snapshots permanently. This is the only genuinely irreversible
/// operation in the pruning system; every caller is expected to have made the
/// user type the count.
pub async fn purge(pool: &PgPool, document_ids: &[String]) -> CoreResult<u64> {
    if document_ids.is_empty() {
        return Ok(0);
    }
    Ok(
        sqlx::query("DELETE FROM ovis.trash_document WHERE document_id = ANY($1)")
            .bind(document_ids.to_vec())
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

/// Ids in the trash, for the reaper's "would this delete be a no-op" check and
/// for bulk selection.
pub async fn ids_matching(pool: &PgPool, filter: &TrashFilter) -> CoreResult<Vec<String>> {
    let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
        "SELECT document_id FROM ovis.trash_document WHERE restored_at IS NULL",
    );
    if let Some(connector_id) = filter.connector_id {
        qb.push(" AND connector_id = ").push_bind(connector_id);
    }
    if let Some(document_id) = &filter.document_id {
        qb.push(" AND document_id = ")
            .push_bind(document_id.clone());
    }
    if let Some(hold) = filter.hold {
        qb.push(" AND hold = ").push_bind(hold);
    }
    if let Some(days) = filter.expiring_within_days {
        qb.push(" AND expires_at <= now() + make_interval(days => ")
            .push_bind(days as i32)
            .push(")");
    }
    qb.push(" ORDER BY document_id");
    Ok(qb.build_query_scalar().fetch_all(pool).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_round_trip_preserves_embedding_scale_values() {
        // Embedding components live in roughly [-1, 1]; f16 has ~3 decimal
        // digits there, far finer than cosine similarity can distinguish.
        for value in [0.0f32, 1.0, -1.0, 0.5, -0.333, 0.007805, 0.12345, -0.98765] {
            let back = f16_bits_to_f32(f32_to_f16_bits(value));
            assert!(
                (back - value).abs() < 0.001,
                "{value} round-tripped to {back}"
            );
        }
    }

    #[test]
    fn f16_handles_zero_infinity_and_overflow_without_panicking() {
        assert_eq!(f16_bits_to_f32(f32_to_f16_bits(0.0)), 0.0);
        assert!(f16_bits_to_f32(f32_to_f16_bits(f32::INFINITY)).is_infinite());
        assert!(f16_bits_to_f32(f32_to_f16_bits(1e30)).is_infinite());
        assert_eq!(f16_bits_to_f32(f32_to_f16_bits(1e-30)), 0.0);
        assert!(f16_bits_to_f32(f32_to_f16_bits(f32::NAN)).is_nan());
    }

    #[test]
    fn vectors_compact_and_expand_back_to_the_same_length() {
        let original: Vec<f32> = (0..768).map(|i| (i as f32 / 768.0) - 0.5).collect();
        let mut source = json!({
            "document_id": "https://a/x",
            "content_vector": original,
            "content": "hello",
        });
        compact_vectors(&mut source);
        assert!(
            source["content_vector"]["__f16_b64"].is_string(),
            "vector should be packed"
        );
        assert_eq!(source["content_vector"]["dim"], 768);
        // Packed form must be far smaller than the float-array rendering.
        let packed_len = source["content_vector"].to_string().len();
        let raw_len = json!(original).to_string().len();
        assert!(
            packed_len < raw_len / 3,
            "packed {packed_len} vs raw {raw_len}"
        );

        expand_vectors(&mut source);
        let restored = source["content_vector"].as_array().unwrap();
        assert_eq!(restored.len(), 768);
        for (i, value) in restored.iter().enumerate() {
            assert!((value.as_f64().unwrap() as f32 - original[i]).abs() < 0.001);
        }
        assert_eq!(source["content"], "hello", "other fields are untouched");
    }

    #[test]
    fn expanding_a_snapshot_without_vectors_is_a_no_op() {
        let mut source = json!({ "content": "hello", "chunk_index": 3 });
        expand_vectors(&mut source);
        assert_eq!(source["content"], "hello");
        assert_eq!(source["chunk_index"], 3);
    }

    /// Restore never interpolates snapshot content into SQL. Column names come
    /// from the live table definition via `jsonb_populate_record`, and the
    /// snapshot travels as a single bound parameter — so a snapshot doctored
    /// in the database cannot become a statement.
    #[test]
    fn restore_binds_the_snapshot_rather_than_building_sql_from_it() {
        let source = include_str!("trash.rs");
        let restore_body = source
            .split("pub async fn restore(")
            .nth(1)
            .expect("restore exists");
        assert!(
            restore_body.contains("jsonb_populate_record"),
            "restore must let Postgres map JSON onto the row type"
        );
        for line in restore_body.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            // A format! that builds an INSERT is the shape being ruled out.
            assert!(
                !(line.contains("format!") && line.to_uppercase().contains("INSERT")),
                "restore must not assemble INSERT statements from snapshot data: {}",
                line.trim()
            );
        }
    }

    #[test]
    fn snapshots_serialise_and_report_their_size() {
        let snapshot = Snapshot {
            version: SNAPSHOT_VERSION,
            document_id: "https://a/x".into(),
            document: json!({ "id": "https://a/x", "semantic_id": "X" }),
            tags: vec![],
            cc_pairs: vec![json!({ "id": "https://a/x", "connector_id": 4 })],
            chunks: vec![json!({ "_id": "https://a/x__0", "_source": { "content": "hi" } })],
            vectors_included: true,
            captured_at: Utc::now(),
        };
        assert_eq!(snapshot.chunk_count(), 1);
        assert!(snapshot.byte_size() > 0);
        assert_eq!(snapshot.semantic_id().as_deref(), Some("X"));
        assert_eq!(snapshot.connector_id(), Some(4));

        let json = serde_json::to_value(&snapshot).unwrap();
        let back: Snapshot = serde_json::from_value(json).unwrap();
        assert_eq!(back.document_id, snapshot.document_id);
        assert_eq!(back.chunks.len(), 1);
    }

    #[test]
    fn ddl_stays_inside_the_ovis_schema() {
        for statement in DDL {
            let lowered = statement.to_lowercase();
            if lowered.starts_with("create table") {
                assert!(lowered.contains("ovis."), "{statement}");
            }
            if lowered.starts_with("create index") {
                assert!(lowered.contains(" on ovis."), "{statement}");
            }
        }
    }

    /// The restore path is the one place in the pruning system allowed to
    /// write Onyx tables, and only these three. Any other target would mean a
    /// restore is mutating something it never captured.
    #[test]
    fn only_the_documented_onyx_tables_are_written_and_only_by_restore() {
        let source = include_str!("trash.rs");
        // Exactly the three tables a snapshot captures. Writing anything else
        // would mean restore is mutating state it never recorded and therefore
        // cannot undo.
        let allowed = [
            "document",
            "document__tag",
            "document_by_connector_credential_pair",
        ];
        for (idx, line) in source.lines().enumerate() {
            let lowered = line.to_lowercase();
            for verb in ["insert into ", "update ", "delete from "] {
                let Some(pos) = lowered.find(verb) else {
                    continue;
                };
                let rest = lowered[pos + verb.len()..].trim_start();
                let Some(target) = rest.strip_prefix("public.") else {
                    continue;
                };
                // The identifier ends at whitespace, an opening paren or a
                // backslash continuation.
                let table: String = target
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                assert!(
                    allowed.contains(&table.as_str()),
                    "line {}: trash writes an unexpected Onyx table `{table}`: {}",
                    idx + 1,
                    line.trim()
                );
            }
        }
    }

    /// Deleting is only safe if the snapshot is already in the same
    /// transaction. Asserted structurally: the cascade call must come after
    /// the INSERT in `trash_and_delete`, with no commit between them.
    #[test]
    fn the_snapshot_insert_precedes_the_cascade_in_one_transaction() {
        let source = include_str!("trash.rs");
        let body = source
            .split("pub async fn trash_and_delete")
            .nth(1)
            .expect("trash_and_delete exists");
        let insert = body
            .find("INSERT INTO ovis.trash_document")
            .expect("inserts");
        let cascade = body.find("delete_document_in_tx").expect("cascades");
        let commit = body.find("tx.commit()").expect("commits");
        assert!(insert < cascade, "the snapshot must be written first");
        assert!(
            cascade < commit,
            "the cascade must be inside the transaction"
        );
        assert!(
            !body[insert..cascade].contains("commit"),
            "no commit may separate the snapshot from the deletion"
        );
    }
}
