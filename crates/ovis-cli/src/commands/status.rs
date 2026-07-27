//! `ovis status` — the one-glance health panel.
//!
//! Exit 0 healthy, 13 degraded, 12 unreachable. The three are genuinely
//! different situations and a script should be able to tell them apart without
//! parsing text.

use ovis_core::api_types::{DependencyHealth, HealthResponse, OnyxHealth};

use crate::ctx::Ctx;
use crate::error::{CliError, CliResult};
use crate::output::style::Tone;
use crate::output::table::{Grid, GridCell};
use crate::output::Format;

pub async fn run(ctx: &Ctx) -> CliResult<()> {
    let (healthy, report) = ctx.api.health().await?;
    render(ctx, healthy, &report)
}

/// Render a health report and decide the exit code. Shared with
/// `ovis server status`, which probes a possibly different URL.
pub fn render(ctx: &Ctx, healthy: bool, report: &HealthResponse) -> CliResult<()> {
    match ctx.out.format {
        Format::Json => ctx.out.json(report)?,
        Format::Yaml => ctx.out.yaml(report)?,
        Format::Ndjson => ctx.out.ndjson(std::slice::from_ref(report))?,
        Format::Table | Format::Csv => {
            let grid = panel(ctx, report);
            ctx.out.grid(&grid)?;
        }
    }

    if ctx.out.format == Format::Table {
        ctx.out.footer(format!(
            "server {} · {}",
            ctx.cfg.server.value, report.version
        ));
    }

    if healthy {
        Ok(())
    } else {
        Err(CliError::Degraded(format!(
            "the server reports status '{}'{}",
            report.status,
            degradation_summary(report)
        )))
    }
}

/// Name the specific thing that is wrong, rather than just saying "degraded".
fn degradation_summary(report: &HealthResponse) -> String {
    let mut reasons = Vec::new();
    if report.postgres.status != "ok" {
        reasons.push(format!("postgres {}", report.postgres.status));
    }
    if report.opensearch.status != "ok" {
        reasons.push(format!("opensearch {}", report.opensearch.status));
    }
    if !report.schema_ok {
        reasons.push(format!(
            "schema mismatch ({} missing column(s))",
            report.missing_columns.len()
        ));
    }
    if !report.unhandled_document_fk_children.is_empty() {
        reasons.push(format!(
            "{} unhandled foreign key(s) onto document",
            report.unhandled_document_fk_children.len()
        ));
    }
    if reasons.is_empty() {
        String::new()
    } else {
        format!(": {}", reasons.join(", "))
    }
}

fn dep_cell(dep: &DependencyHealth) -> GridCell {
    let tone = match dep.status.as_str() {
        "ok" => Tone::Ok,
        "not_configured" | "unconfigured" => Tone::Dim,
        _ => Tone::Error,
    };
    let mut text = dep.status.clone();
    if let Some(latency) = dep.latency_ms {
        text.push_str(&format!("  {latency:.1} ms"));
    }
    if let Some(detail) = &dep.detail {
        if !detail.trim().is_empty() {
            text.push_str(&format!("  ({})", detail.trim()));
        }
    }
    GridCell::toned(text, tone)
}

fn onyx_cell(onyx: &OnyxHealth) -> GridCell {
    if !onyx.configured {
        // Not an error: reads work fine without a token, actions do not.
        return GridCell::toned(
            "not configured — connector actions will answer 503 ONYX_UNCONFIGURED. Mint a \
             token with `ovis server setup-onyx-key`",
            Tone::Dim,
        );
    }
    let tone = match onyx.status.as_str() {
        "ok" => Tone::Ok,
        "unauthorized" => Tone::Error,
        _ => Tone::Warn,
    };
    let mut text = onyx.status.clone();
    if let Some(version) = &onyx.version {
        text.push_str(&format!("  {version}"));
    }
    if let Some(latency) = onyx.latency_ms {
        text.push_str(&format!("  {latency:.1} ms"));
    }
    if onyx.status == "unauthorized" {
        text.push_str("  — the token was rejected; mint a new one");
    }
    if let Some(detail) = &onyx.detail {
        if !detail.trim().is_empty() {
            text.push_str(&format!("  ({})", detail.trim()));
        }
    }
    GridCell::toned(text, tone)
}

fn panel(ctx: &Ctx, report: &HealthResponse) -> Grid {
    let mut grid = Grid::new(vec!["COMPONENT".into(), "STATE".into()]);
    let mut row = |k: &str, v: GridCell| grid.push(vec![GridCell::plain(k), v]);

    row(
        "overall",
        GridCell::toned(
            &report.status,
            if report.status == "ok" {
                Tone::Ok
            } else {
                Tone::Error
            },
        ),
    );
    row("postgres", dep_cell(&report.postgres));
    row("opensearch", dep_cell(&report.opensearch));
    row("onyx api", onyx_cell(&report.onyx_api));
    row("embedder", dep_cell(&report.embedder));
    row("index", GridCell::plain(&report.index_name));

    row(
        "schema",
        if report.schema_ok {
            GridCell::toned("ok", Tone::Ok)
        } else {
            GridCell::toned(
                format!("missing columns: {}", report.missing_columns.join(", ")),
                Tone::Error,
            )
        },
    );

    if !report.unhandled_document_fk_children.is_empty() {
        // A restricting FK OVIS does not clear means a cascading delete would
        // fail half way through — worth shouting about.
        row(
            "delete safety",
            GridCell::toned(
                format!(
                    "unhandled foreign keys onto document: {}",
                    report.unhandled_document_fk_children.join(", ")
                ),
                Tone::Error,
            ),
        );
    }

    row(
        "support indexes",
        if report.missing_indexes.is_empty() {
            GridCell::toned("all present", Tone::Ok)
        } else {
            // A performance warning, never an error: the server works without
            // them, just slowly.
            GridCell::toned(
                format!(
                    "{} missing ({}) — apply ops/onyx_indexes.sql",
                    report.missing_indexes.len(),
                    report.missing_indexes.join(", ")
                ),
                Tone::Warn,
            )
        },
    );

    let _ = ctx;
    grid
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dep(status: &str) -> DependencyHealth {
        DependencyHealth {
            status: status.into(),
            latency_ms: Some(6.8),
            detail: None,
        }
    }

    fn report(status: &str) -> HealthResponse {
        HealthResponse {
            status: status.into(),
            postgres: dep("ok"),
            opensearch: dep("ok"),
            onyx_api: OnyxHealth {
                configured: true,
                status: "ok".into(),
                latency_ms: Some(8.6),
                version: Some("v4.3.4".into()),
                detail: None,
            },
            embedder: dep("ok"),
            schema_ok: true,
            missing_columns: vec![],
            unhandled_document_fk_children: vec![],
            missing_indexes: vec![],
            index_name: "danswer_chunk_snowflake_arctic_embed_m".into(),
            version: "0.2.0".into(),
        }
    }

    #[test]
    fn a_healthy_report_names_no_reasons() {
        assert_eq!(degradation_summary(&report("ok")), "");
    }

    #[test]
    fn a_degraded_report_names_the_specific_failure() {
        let mut r = report("degraded");
        r.postgres = dep("down");
        let summary = degradation_summary(&r);
        assert!(summary.contains("postgres down"), "{summary}");

        let mut r = report("degraded");
        r.schema_ok = false;
        r.missing_columns = vec!["document.chunk_count".into()];
        assert!(degradation_summary(&r).contains("schema mismatch"));
    }

    #[test]
    fn an_unconfigured_onyx_is_dim_not_an_error() {
        // Reads work without a token; only actions need one.
        let cell = onyx_cell(&OnyxHealth {
            configured: false,
            status: "not_configured".into(),
            latency_ms: None,
            version: None,
            detail: None,
        });
        assert_eq!(cell.tone, Tone::Dim);
        assert!(cell.text.contains("setup-onyx-key"));
    }

    #[test]
    fn a_rejected_onyx_token_is_an_error_that_says_what_to_do() {
        let cell = onyx_cell(&OnyxHealth {
            configured: true,
            status: "unauthorized".into(),
            latency_ms: None,
            version: None,
            detail: None,
        });
        assert_eq!(cell.tone, Tone::Error);
        assert!(cell.text.contains("mint a new one"));
    }

    #[test]
    fn missing_support_indexes_are_a_warning_not_a_failure() {
        let mut r = report("ok");
        r.missing_indexes = vec!["ix_ovis_document_updated_desc".into()];
        // Still healthy overall, so no reason is added.
        assert_eq!(degradation_summary(&r), "");
    }

    #[test]
    fn unhandled_foreign_keys_are_reported_because_a_delete_would_fail_midway() {
        let mut r = report("degraded");
        r.unhandled_document_fk_children = vec!["document__tag".into()];
        assert!(degradation_summary(&r).contains("foreign key"));
    }
}
