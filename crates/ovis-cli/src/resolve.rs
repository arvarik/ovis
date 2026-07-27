//! Turning what a human typed into what the API wants.
//!
//! Two ids matter and they are not interchangeable: `connector_id` is what
//! `GET /pages?connector_id=` filters on, and `cc_pair_id` is what every
//! `/connectors/{id}` path and action uses. A number typed on the command line
//! is therefore ambiguous, and guessing wrong means acting on the wrong
//! connector — so a number is looked up rather than assumed.

use ovis_core::api_types::ConnectorSummary;

use crate::ctx::Ctx;
use crate::error::{CliError, CliResult};

/// Parse a `--since`/`--until` value: `2h`, `3d`, `2026-07-01`, or a full
/// RFC3339 timestamp.
pub fn parse_when(raw: &str) -> Result<chrono::DateTime<chrono::Utc>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("empty time value".into());
    }

    // A duration means "that long ago", which is how everyone reads `--since 2h`.
    if let Ok(duration) = humantime::parse_duration(trimmed) {
        let delta = chrono::Duration::from_std(duration)
            .map_err(|_| format!("'{trimmed}' is too large a duration"))?;
        return Ok(chrono::Utc::now() - delta);
    }

    if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(trimmed) {
        return Ok(ts.with_timezone(&chrono::Utc));
    }

    // A bare date means midnight UTC on that date.
    if let Ok(date) = chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        return Ok(chrono::DateTime::from_naive_utc_and_offset(
            date.and_hms_opt(0, 0, 0).expect("midnight is a valid time"),
            chrono::Utc,
        ));
    }

    Err(format!(
        "cannot read '{trimmed}' as a time; try 2h, 3d, 2026-07-01, or a full RFC3339 timestamp"
    ))
}

/// Parse `--chunks MIN..MAX`, where either end may be omitted.
pub fn parse_chunk_range(raw: &str) -> Result<(Option<i32>, Option<i32>), String> {
    let trimmed = raw.trim();
    let Some((lo, hi)) = trimmed.split_once("..") else {
        // A bare number is an exact count, which is the obvious reading.
        let exact: i32 = trimmed.parse().map_err(|_| {
            format!("cannot read '{trimmed}' as a chunk range; try 1..5, ..0 or 20..")
        })?;
        return Ok((Some(exact), Some(exact)));
    };

    let parse_end = |s: &str, which: &str| -> Result<Option<i32>, String> {
        let s = s.trim();
        if s.is_empty() {
            return Ok(None);
        }
        s.parse::<i32>()
            .map(Some)
            .map_err(|_| format!("cannot read '{s}' as the {which} of a chunk range"))
    };

    let min = parse_end(lo, "start")?;
    let max = parse_end(hi, "end")?;
    if min.is_none() && max.is_none() {
        return Err("'..' bounds nothing; give at least one end".into());
    }
    if let (Some(min), Some(max)) = (min, max) {
        if min > max {
            return Err(format!("chunk range {min}..{max} is inverted"));
        }
    }
    Ok((min, max))
}

/// Parse `--sort updated:desc` into the API's `sort` value.
pub fn parse_sort(raw: &str) -> Result<String, String> {
    let (field, dir) = match raw.split_once(':') {
        Some((f, d)) => (f.trim(), Some(d.trim())),
        None => (raw.trim(), None),
    };

    let field = match field.to_ascii_lowercase().as_str() {
        "updated" | "updated_at" | "recency" => "updated",
        "chunks" | "chunk_count" => "chunks",
        "id" => "id",
        "boost" => "boost",
        other => {
            return Err(format!(
                "unknown sort field '{other}'; expected updated, chunks, id or boost"
            ))
        }
    };

    let dir = match dir.map(|d| d.to_ascii_lowercase()) {
        None => "desc",
        Some(d) if d == "asc" => "asc",
        Some(d) if d == "desc" => "desc",
        Some(other) => {
            return Err(format!(
                "unknown sort direction '{other}'; expected asc or desc"
            ))
        }
    };

    // `id` orders lexicographically, where ascending is the natural reading.
    Ok(format!("{field}_{dir}"))
}

/// A connector, resolved from whatever the user typed.
#[derive(Debug, Clone)]
pub struct ResolvedConnector {
    pub summary: ConnectorSummary,
}

impl ResolvedConnector {
    pub fn cc_pair_id(&self) -> i32 {
        self.summary.cc_pair_id
    }
    pub fn connector_id(&self) -> i32 {
        self.summary.connector_id
    }
    pub fn name(&self) -> &str {
        &self.summary.name
    }
}

/// Resolve `ID|NAME` against the live connector list.
///
/// Matching order: exact name, case-insensitive name, `cc_pair_id`,
/// `connector_id`, then a unique substring. Anything ambiguous is an error that
/// lists the candidates rather than a guess.
pub async fn connector(ctx: &Ctx, reference: &str) -> CliResult<ResolvedConnector> {
    let all = ctx.api.connectors().await?;
    resolve_connector_in(&all, reference).map(|summary| ResolvedConnector { summary })
}

pub fn resolve_connector_in(
    all: &[ConnectorSummary],
    reference: &str,
) -> CliResult<ConnectorSummary> {
    let needle = reference.trim();
    if needle.is_empty() {
        return Err(CliError::Usage("no connector given".into()));
    }

    if let Some(hit) = all.iter().find(|c| c.name == needle) {
        return Ok(hit.clone());
    }
    if let Some(hit) = all.iter().find(|c| c.name.eq_ignore_ascii_case(needle)) {
        return Ok(hit.clone());
    }

    if let Ok(number) = needle.parse::<i32>() {
        // cc_pair_id first: it is what every /connectors path and action takes,
        // so it is the number a user is most likely holding.
        if let Some(hit) = all.iter().find(|c| c.cc_pair_id == number) {
            return Ok(hit.clone());
        }
        if let Some(hit) = all.iter().find(|c| c.connector_id == number) {
            return Ok(hit.clone());
        }
        return Err(CliError::Api(crate::error::ApiErrorBody {
            code: "NOT_FOUND".into(),
            message: format!("no connector with cc-pair id or connector id {number}"),
            status: 404,
            req_id: String::new(),
        }));
    }

    let lower = needle.to_ascii_lowercase();
    let matches: Vec<&ConnectorSummary> = all
        .iter()
        .filter(|c| c.name.to_ascii_lowercase().contains(&lower))
        .collect();

    match matches.len() {
        1 => Ok(matches[0].clone()),
        0 => Err(CliError::Api(crate::error::ApiErrorBody {
            code: "NOT_FOUND".into(),
            message: format!("no connector matching '{needle}'"),
            status: 404,
            req_id: String::new(),
        })),
        _ => {
            let mut listed: Vec<String> = matches
                .iter()
                .take(10)
                .map(|c| format!("  {} (cc-pair {})", c.name, c.cc_pair_id))
                .collect();
            if matches.len() > 10 {
                listed.push(format!("  … and {} more", matches.len() - 10));
            }
            Err(CliError::Usage(format!(
                "'{needle}' matches {} connectors:\n{}",
                matches.len(),
                listed.join("\n")
            )))
        }
    }
}

/// Normalise a `--source` value. Onyx stores these upper-case; the API compares
/// case-insensitively, so this is only about catching typos early.
pub fn normalise_source(raw: &str) -> String {
    raw.trim().to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn summary(cc: i32, conn: i32, name: &str) -> ConnectorSummary {
        ConnectorSummary {
            connector_id: conn,
            cc_pair_id: cc,
            name: name.into(),
            source: "WEB".into(),
            status: "ACTIVE".into(),
            parked: false,
            in_repeated_error_state: false,
            doc_count: 0,
            last_successful_index_time: None,
            refresh_freq_secs: None,
            indexing_trigger: None,
            last_attempt: None,
        }
    }

    fn fleet() -> Vec<ConnectorSummary> {
        vec![
            summary(5, 4, "tildes"),
            summary(6, 7, "stanford-philosophy"),
            summary(7, 9, "stanford-encyclopedia"),
            // A name that is also a number, to prove the lookup order.
            summary(9, 11, "42"),
        ]
    }

    #[test]
    fn an_exact_name_wins_over_everything_else() {
        let hit = resolve_connector_in(&fleet(), "tildes").unwrap();
        assert_eq!(hit.cc_pair_id, 5);
        // Even when the name looks like a number belonging to someone else.
        let hit = resolve_connector_in(&fleet(), "42").unwrap();
        assert_eq!(hit.cc_pair_id, 9);
    }

    #[test]
    fn names_match_case_insensitively() {
        assert_eq!(
            resolve_connector_in(&fleet(), "TILDES").unwrap().cc_pair_id,
            5
        );
    }

    #[test]
    fn a_number_resolves_by_cc_pair_first_then_connector_id() {
        // 7 is cc_pair 7 (stanford-encyclopedia) and connector_id 7
        // (stanford-philosophy). cc-pair wins: it is what the action paths take.
        let hit = resolve_connector_in(&fleet(), "7").unwrap();
        assert_eq!(hit.name, "stanford-encyclopedia");

        // 11 is only a connector_id, so it still resolves.
        let hit = resolve_connector_in(&fleet(), "11").unwrap();
        assert_eq!(hit.name, "42");
    }

    #[test]
    fn an_unknown_number_is_not_found_rather_than_a_substring_search() {
        let err = resolve_connector_in(&fleet(), "999").unwrap_err();
        assert_eq!(err.exit_code(), crate::error::exit::NOT_FOUND);
    }

    #[test]
    fn an_ambiguous_substring_lists_the_candidates_instead_of_guessing() {
        let err = resolve_connector_in(&fleet(), "stanford").unwrap_err();
        let msg = err.message();
        assert!(msg.contains("stanford-philosophy"), "{msg}");
        assert!(msg.contains("stanford-encyclopedia"), "{msg}");
        assert_eq!(err.exit_code(), crate::error::exit::USAGE);
    }

    #[test]
    fn a_unique_substring_resolves() {
        assert_eq!(
            resolve_connector_in(&fleet(), "philos").unwrap().cc_pair_id,
            6
        );
    }

    #[test]
    fn durations_are_read_as_time_ago() {
        let two_hours = parse_when("2h").unwrap();
        let delta = Utc::now().signed_duration_since(two_hours).num_minutes();
        assert!((119..=121).contains(&delta), "got {delta} minutes");

        let three_days = parse_when("3d").unwrap();
        assert_eq!(Utc::now().signed_duration_since(three_days).num_days(), 3);
    }

    #[test]
    fn absolute_dates_and_timestamps_parse() {
        assert_eq!(
            parse_when("2026-07-01").unwrap().to_rfc3339(),
            "2026-07-01T00:00:00+00:00"
        );
        assert_eq!(
            parse_when("2026-07-01T12:00:00Z").unwrap().to_rfc3339(),
            "2026-07-01T12:00:00+00:00"
        );
        assert!(parse_when("last tuesday").is_err());
    }

    #[test]
    fn chunk_ranges_accept_open_ends() {
        assert_eq!(parse_chunk_range("1..5").unwrap(), (Some(1), Some(5)));
        assert_eq!(parse_chunk_range("..0").unwrap(), (None, Some(0)));
        assert_eq!(parse_chunk_range("20..").unwrap(), (Some(20), None));
        assert_eq!(parse_chunk_range("7").unwrap(), (Some(7), Some(7)));
    }

    #[test]
    fn an_inverted_or_empty_chunk_range_is_rejected_rather_than_returning_nothing() {
        assert!(parse_chunk_range("5..1").is_err());
        assert!(parse_chunk_range("..").is_err());
        assert!(parse_chunk_range("a..b").is_err());
    }

    #[test]
    fn sort_values_map_to_the_api_vocabulary_and_default_to_descending() {
        assert_eq!(parse_sort("updated").unwrap(), "updated_desc");
        assert_eq!(parse_sort("chunks:asc").unwrap(), "chunks_asc");
        assert_eq!(parse_sort("BOOST:DESC").unwrap(), "boost_desc");
        assert_eq!(parse_sort("id:asc").unwrap(), "id_asc");
    }

    #[test]
    fn a_mistyped_sort_names_the_valid_fields() {
        let err = parse_sort("chunk").unwrap_err();
        assert!(err.contains("chunks"), "{err}");
        assert!(parse_sort("updated:sideways").is_err());
    }

    #[test]
    fn sources_are_normalised_to_how_onyx_stores_them() {
        assert_eq!(normalise_source(" web "), "WEB");
        assert_eq!(normalise_source("ingestion_api"), "INGESTION_API");
    }
}
