//! Aggregate stats: crawl rate, activity timeline, per-source breakdown.
//!
//! Everything here is cached by the caller. The timeline and per-source queries
//! touch millions of rows; they are dashboard furniture, not per-keystroke
//! paths, and they say so by living behind a TTL.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

use crate::api_types::{SourceStat, TimelineBucket};
use crate::error::{CoreError, CoreResult};

/// Documents touched in the last 15 minutes — the signal the homelab ops scripts
/// watch to tell "crawling" from "wedged". Served by `ix_document_last_modified`.
pub async fn docs_since(pool: &PgPool, minutes: i64) -> CoreResult<i64> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.document \
         WHERE last_modified > now() - make_interval(mins => $1::int)",
    )
    .bind(minutes as i32)
    .fetch_one(pool)
    .await?;
    Ok(count)
}

/// Time window accepted by `/stats/timeline`. An enum rather than a string so no
/// caller text reaches the interval expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineWindow {
    Day,
    Week,
    Month,
}

impl TimelineWindow {
    pub fn as_str(self) -> &'static str {
        match self {
            TimelineWindow::Day => "24h",
            TimelineWindow::Week => "7d",
            TimelineWindow::Month => "30d",
        }
    }

    fn hours(self) -> i32 {
        match self {
            TimelineWindow::Day => 24,
            TimelineWindow::Week => 24 * 7,
            TimelineWindow::Month => 24 * 30,
        }
    }

    /// Sensible bucket when the caller does not pick one.
    pub fn default_bucket(self) -> TimelineBucketSize {
        match self {
            TimelineWindow::Day => TimelineBucketSize::Hour,
            TimelineWindow::Week | TimelineWindow::Month => TimelineBucketSize::Day,
        }
    }
}

impl std::str::FromStr for TimelineWindow {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "24h" => Ok(TimelineWindow::Day),
            "7d" => Ok(TimelineWindow::Week),
            "30d" => Ok(TimelineWindow::Month),
            other => Err(CoreError::Invalid(format!(
                "unknown window '{other}'; expected 24h, 7d or 30d"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineBucketSize {
    Hour,
    Day,
}

impl TimelineBucketSize {
    pub fn as_str(self) -> &'static str {
        match self {
            TimelineBucketSize::Hour => "1h",
            TimelineBucketSize::Day => "1d",
        }
    }

    fn trunc_unit(self) -> &'static str {
        match self {
            TimelineBucketSize::Hour => "hour",
            TimelineBucketSize::Day => "day",
        }
    }
}

impl std::str::FromStr for TimelineBucketSize {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "1h" => Ok(TimelineBucketSize::Hour),
            "1d" => Ok(TimelineBucketSize::Day),
            other => Err(CoreError::Invalid(format!(
                "unknown bucket '{other}'; expected 1h or 1d"
            ))),
        }
    }
}

/// Crawl-activity histogram over `document.last_modified`.
///
/// Buckets with no documents are filled in with zero, so a chart does not have
/// to guess at gaps.
pub async fn timeline(
    pool: &PgPool,
    window: TimelineWindow,
    bucket: TimelineBucketSize,
) -> CoreResult<Vec<TimelineBucket>> {
    let sql = format!(
        "WITH buckets AS ( \
             SELECT generate_series( \
                 date_trunc('{unit}', now() - make_interval(hours => $1::int)), \
                 date_trunc('{unit}', now()), \
                 '1 {unit}'::interval \
             ) AS bucket \
         ), \
         counted AS ( \
             SELECT date_trunc('{unit}', last_modified) AS bucket, count(*) AS docs \
             FROM public.document \
             WHERE last_modified > now() - make_interval(hours => $1::int) \
             GROUP BY 1 \
         ) \
         SELECT b.bucket, COALESCE(c.docs, 0)::bigint AS docs \
         FROM buckets b LEFT JOIN counted c ON c.bucket = b.bucket \
         ORDER BY b.bucket",
        unit = bucket.trunc_unit()
    );

    let rows = sqlx::query(&sql)
        .bind(window.hours())
        .fetch_all(pool)
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| TimelineBucket {
            bucket: r.get::<DateTime<Utc>, _>("bucket"),
            docs: r.get("docs"),
        })
        .collect())
}

/// Documents and connectors per `connector.source`.
///
/// `count(DISTINCT dcc.id)` rather than `count(*)`: a document attached to two
/// connectors of the same source is one document, and 3.2k documents have
/// multiple connectors. Chunk counts per source come from OpenSearch, not here.
pub async fn by_source(pool: &PgPool) -> CoreResult<Vec<SourceStat>> {
    let rows = sqlx::query(
        "SELECT c.source, \
                count(DISTINCT c.id) AS connectors, \
                count(DISTINCT dcc.id) AS documents \
         FROM public.connector c \
         LEFT JOIN public.document_by_connector_credential_pair dcc ON dcc.connector_id = c.id \
         GROUP BY c.source \
         ORDER BY documents DESC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SourceStat {
            source: r.get("source"),
            connectors: r.get("connectors"),
            documents: r.get("documents"),
            chunks: None,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_units_come_from_the_enum_only() {
        for bucket in [TimelineBucketSize::Hour, TimelineBucketSize::Day] {
            let unit = bucket.trunc_unit();
            assert!(
                unit.chars().all(|c| c.is_ascii_lowercase()),
                "bucket unit must be a bare identifier, got {unit}"
            );
        }
    }

    #[test]
    fn window_and_bucket_parsing_round_trips() {
        for w in [
            TimelineWindow::Day,
            TimelineWindow::Week,
            TimelineWindow::Month,
        ] {
            assert_eq!(w.as_str().parse::<TimelineWindow>().unwrap(), w);
        }
        for b in [TimelineBucketSize::Hour, TimelineBucketSize::Day] {
            assert_eq!(b.as_str().parse::<TimelineBucketSize>().unwrap(), b);
        }
        assert!("1w".parse::<TimelineBucketSize>().is_err());
        assert!("1y".parse::<TimelineWindow>().is_err());
    }

    #[test]
    fn default_bucket_keeps_charts_readable() {
        // 30 days of hourly buckets would be 720 points for a sparkline.
        assert_eq!(
            TimelineWindow::Day.default_bucket(),
            TimelineBucketSize::Hour
        );
        assert_eq!(
            TimelineWindow::Month.default_bucket(),
            TimelineBucketSize::Day
        );
    }
}
