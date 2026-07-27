//! Sort orders and opaque keyset cursors.
//!
//! Offset pagination is kept for the numbered-page UI but bounded; deep paging
//! and every streaming/CLI path uses keyset cursors, which are O(1) in page
//! depth instead of materialising and discarding `OFFSET n` rows.
//!
//! A cursor is `base64url(json)` of the last row's sort key. It is opaque to
//! clients: they echo `next_cursor` back and never construct one. Every cursor
//! records which sort produced it, so pairing a cursor with a different `sort`
//! is rejected instead of silently returning nonsense.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::api_types::PageListItem;
use crate::error::{CoreError, CoreResult};

/// Sort orders for `GET /pages`. Each one has an index in
/// `ops/onyx_indexes.sql` that matches it exactly, including tie-break
/// direction, so the database never sorts more rows than the page returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    /// Newest first by `COALESCE(doc_updated_at, last_modified)`.
    #[default]
    UpdatedDesc,
    UpdatedAsc,
    ChunksDesc,
    ChunksAsc,
    IdAsc,
    IdDesc,
    BoostDesc,
}

impl SortOrder {
    pub fn as_str(self) -> &'static str {
        match self {
            SortOrder::UpdatedDesc => "updated_desc",
            SortOrder::UpdatedAsc => "updated_asc",
            SortOrder::ChunksDesc => "chunks_desc",
            SortOrder::ChunksAsc => "chunks_asc",
            SortOrder::IdAsc => "id_asc",
            SortOrder::IdDesc => "id_desc",
            SortOrder::BoostDesc => "boost_desc",
        }
    }

    /// The `ORDER BY` clause body. Static text derived from the enum — no
    /// caller-supplied string ever reaches SQL through this path.
    ///
    /// `sort_ts` is the `COALESCE(doc_updated_at, last_modified)` output column
    /// and is `NOT NULL` (because `last_modified` is), so the recency sorts need
    /// no NULL handling. `chunk_count` *is* nullable, hence the explicit
    /// `NULLS LAST` and the null-tail handling in the keyset predicate.
    pub fn order_by(self) -> &'static str {
        match self {
            SortOrder::UpdatedDesc => "sort_ts DESC, d.id DESC",
            SortOrder::UpdatedAsc => "sort_ts ASC, d.id ASC",
            SortOrder::ChunksDesc => "d.chunk_count DESC NULLS LAST, d.id DESC",
            SortOrder::ChunksAsc => "d.chunk_count ASC NULLS LAST, d.id DESC",
            SortOrder::IdAsc => "d.id ASC",
            SortOrder::IdDesc => "d.id DESC",
            SortOrder::BoostDesc => "d.boost DESC, d.id DESC",
        }
    }
}

impl std::str::FromStr for SortOrder {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "updated_desc" => Ok(SortOrder::UpdatedDesc),
            "updated_asc" => Ok(SortOrder::UpdatedAsc),
            "chunks_desc" => Ok(SortOrder::ChunksDesc),
            "chunks_asc" => Ok(SortOrder::ChunksAsc),
            "id_asc" => Ok(SortOrder::IdAsc),
            "id_desc" => Ok(SortOrder::IdDesc),
            "boost_desc" => Ok(SortOrder::BoostDesc),
            other => Err(CoreError::Invalid(format!(
                "unknown sort '{other}'; expected one of updated_desc, updated_asc, \
                 chunks_desc, chunks_asc, id_asc, id_desc, boost_desc"
            ))),
        }
    }
}

/// The decoded position of the last row of a page.
///
/// Field names are single letters to keep the base64 short — a cursor rides in
/// every SSE reconnect and every CLI page fetch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cursor {
    #[serde(rename = "s")]
    pub sort: SortOrder,
    /// Sort timestamp of the last row (recency sorts only).
    #[serde(rename = "t", default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<DateTime<Utc>>,
    /// Numeric sort key of the last row: `chunk_count` (nullable) or `boost`.
    /// `None` for the chunk sorts means the page ended inside the
    /// null-`chunk_count` tail.
    #[serde(rename = "n", default, skip_serializing_if = "Option::is_none")]
    pub n: Option<i32>,
    #[serde(rename = "i")]
    pub id: String,
}

impl Cursor {
    /// Build the cursor that resumes *after* `item` under `sort`.
    pub fn after(sort: SortOrder, item: &PageListItem) -> Self {
        let (ts, n) = match sort {
            SortOrder::UpdatedDesc | SortOrder::UpdatedAsc => (Some(item.updated_at), None),
            SortOrder::ChunksDesc | SortOrder::ChunksAsc => (None, item.chunk_count),
            SortOrder::BoostDesc => (None, Some(item.boost)),
            SortOrder::IdAsc | SortOrder::IdDesc => (None, None),
        };
        Cursor {
            sort,
            ts,
            n,
            id: item.id.clone(),
        }
    }

    pub fn encode(&self) -> String {
        // serde_json on a struct of primitives cannot fail.
        let json = serde_json::to_vec(self).expect("cursor serialises");
        URL_SAFE_NO_PAD.encode(json)
    }

    /// Decode and validate a client-supplied cursor against the sort it is being
    /// used with. Any tampering, truncation, or sort mismatch is a 400, not a
    /// silently wrong page.
    pub fn decode(token: &str, expected_sort: SortOrder) -> CoreResult<Self> {
        let bytes = URL_SAFE_NO_PAD
            .decode(token.as_bytes())
            .map_err(|_| CoreError::Invalid("cursor is not valid base64url".into()))?;
        let cursor: Cursor = serde_json::from_slice(&bytes)
            .map_err(|_| CoreError::Invalid("cursor payload is malformed".into()))?;

        if cursor.sort != expected_sort {
            return Err(CoreError::Invalid(format!(
                "cursor was issued for sort '{}' but the request asked for '{}'; \
                 restart paging when changing sort",
                cursor.sort.as_str(),
                expected_sort.as_str()
            )));
        }
        if cursor.id.is_empty() {
            return Err(CoreError::Invalid("cursor is missing its id key".into()));
        }
        match expected_sort {
            SortOrder::UpdatedDesc | SortOrder::UpdatedAsc if cursor.ts.is_none() => Err(
                CoreError::Invalid("cursor is missing its timestamp key".into()),
            ),
            SortOrder::BoostDesc if cursor.n.is_none() => {
                Err(CoreError::Invalid("cursor is missing its boost key".into()))
            }
            _ => Ok(cursor),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, ts: &str, chunks: Option<i32>, boost: i32) -> PageListItem {
        PageListItem {
            id: id.into(),
            semantic_id: "t".into(),
            link: None,
            updated_at: ts.parse().unwrap(),
            doc_updated_at: None,
            last_modified: ts.parse().unwrap(),
            chunk_count: chunks,
            boost,
            hidden: false,
            connector_id: None,
            connector_name: None,
            connector_source: None,
            metadata: None,
        }
    }

    #[test]
    fn round_trips_every_sort() {
        let it = item("https://a/b?x=1&y=2", "2026-07-26T10:11:12Z", Some(14), 3);
        for sort in [
            SortOrder::UpdatedDesc,
            SortOrder::UpdatedAsc,
            SortOrder::ChunksDesc,
            SortOrder::ChunksAsc,
            SortOrder::IdAsc,
            SortOrder::IdDesc,
            SortOrder::BoostDesc,
        ] {
            let c = Cursor::after(sort, &it);
            let decoded = Cursor::decode(&c.encode(), sort).expect("decodes");
            assert_eq!(c, decoded, "round-trip failed for {}", sort.as_str());
            assert_eq!(decoded.id, it.id);
        }
    }

    #[test]
    fn encoding_is_url_safe_and_unpadded() {
        let it = item("https://a/b", "2026-07-26T10:11:12Z", Some(1), 0);
        let token = Cursor::after(SortOrder::UpdatedDesc, &it).encode();
        assert!(!token.contains('+'), "{token}");
        assert!(!token.contains('/'), "{token}");
        assert!(!token.contains('='), "{token}");
    }

    #[test]
    fn null_chunk_count_survives_the_round_trip() {
        let it = item("https://a/b", "2026-07-26T10:11:12Z", None, 0);
        let c = Cursor::after(SortOrder::ChunksDesc, &it);
        assert_eq!(c.n, None, "the null tail must be representable");
        let decoded = Cursor::decode(&c.encode(), SortOrder::ChunksDesc).unwrap();
        assert_eq!(decoded.n, None);
    }

    #[test]
    fn rejects_sort_mismatch() {
        let it = item("https://a/b", "2026-07-26T10:11:12Z", Some(1), 0);
        let token = Cursor::after(SortOrder::UpdatedDesc, &it).encode();
        let err = Cursor::decode(&token, SortOrder::ChunksDesc).unwrap_err();
        assert!(matches!(err, CoreError::Invalid(_)));
        assert!(err.to_string().contains("updated_desc"));
    }

    #[test]
    fn rejects_garbage_and_truncation() {
        assert!(Cursor::decode("!!!not base64!!!", SortOrder::UpdatedDesc).is_err());
        assert!(Cursor::decode(
            &URL_SAFE_NO_PAD.encode(b"{\"nope\":1}"),
            SortOrder::UpdatedDesc
        )
        .is_err());
        // Valid JSON, valid sort, but no timestamp for a recency sort.
        let bad = URL_SAFE_NO_PAD.encode(br#"{"s":"updated_desc","i":"x"}"#);
        let err = Cursor::decode(&bad, SortOrder::UpdatedDesc).unwrap_err();
        assert!(err.to_string().contains("timestamp"));
    }

    #[test]
    fn sort_parsing_is_exhaustive_and_symmetric() {
        for sort in [
            SortOrder::UpdatedDesc,
            SortOrder::UpdatedAsc,
            SortOrder::ChunksDesc,
            SortOrder::ChunksAsc,
            SortOrder::IdAsc,
            SortOrder::IdDesc,
            SortOrder::BoostDesc,
        ] {
            assert_eq!(sort.as_str().parse::<SortOrder>().unwrap(), sort);
        }
        assert!("chunk_desc".parse::<SortOrder>().is_err());
        assert_eq!(SortOrder::default(), SortOrder::UpdatedDesc);
    }

    #[test]
    fn order_by_clauses_always_end_in_a_unique_tiebreak() {
        // Without a unique final key, keyset pagination can skip or repeat rows.
        for sort in [
            SortOrder::UpdatedDesc,
            SortOrder::UpdatedAsc,
            SortOrder::ChunksDesc,
            SortOrder::ChunksAsc,
            SortOrder::IdAsc,
            SortOrder::IdDesc,
            SortOrder::BoostDesc,
        ] {
            let clause = sort.order_by();
            let last = clause.rsplit(',').next().unwrap().trim();
            assert!(
                last.starts_with("d.id"),
                "{} must tie-break on the primary key, got '{}'",
                sort.as_str(),
                last
            );
        }
    }
}
