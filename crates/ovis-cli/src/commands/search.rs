//! `ovis search` — content search across the chunk index.
//!
//! On this deployment `mode=semantic|hybrid` always fall back to BM25 and the
//! response says `degraded: "no_knn_field"`. That value is surfaced rather than
//! swallowed: a vector mode that silently behaves like keyword search is worse
//! than one that says it could not run.

use ovis_core::api_types::SearchResponse;

use crate::api::QueryBuilder;
use crate::cli::SearchArgs;
use crate::ctx::Ctx;
use crate::error::{usage, CliResult};
use crate::handles::{self, HandleItem, HandleKind};
use crate::output::style::Tone;
use crate::output::{thousands, Format};
use crate::render;
use crate::resolve;

/// Plain-English readings of the `degraded` values seen so far. The field is an
/// open string — three values exist today and more may appear — so an unknown
/// one is shown verbatim rather than dropped.
pub fn explain_degraded(value: &str) -> String {
    match value {
        "no_knn_field" => {
            "no_knn_field — this index declares a kNN field that no document populates, so \
             vector search cannot run and these results are BM25 keyword matches"
                .to_string()
        }
        "no_embedder" => {
            "no_embedder — the embedding endpoint is unset or unreachable, so these results \
             are BM25 keyword matches"
                .to_string()
        }
        "connector_filter_post_applied" => {
            "connector_filter_post_applied — the chunk index carries no connector field, so \
             the filter was applied after ranking; the total is not exact and a small \
             connector may legitimately show nothing"
                .to_string()
        }
        other => format!("{other} — the server reported a degraded search"),
    }
}

pub async fn run(ctx: &Ctx, args: &SearchArgs) -> CliResult<()> {
    render::validate_columns(ctx, render::SEARCH_COLUMNS)?;
    let query = args.query.join(" ");
    let query = query.trim();
    if query.is_empty() {
        return usage("a search needs a query");
    }

    let mode = match args.mode.to_ascii_lowercase().as_str() {
        m @ ("keyword" | "semantic" | "hybrid") => m.to_string(),
        other => {
            return usage(format!(
                "unknown search mode '{other}'; expected keyword, semantic or hybrid"
            ))
        }
    };

    let mut params = QueryBuilder::new();
    params.push("q", query).push("mode", &mode);

    if let Some(reference) = &args.connector {
        let resolved = resolve::connector(ctx, reference).await?;
        params.push("connector_id", resolved.connector_id());
    }
    if let Some(source) = &args.source {
        params.push("source", resolve::normalise_source(source));
    }
    if args.include_hidden {
        params.push("include_hidden", true);
    }
    params.push(
        "limit",
        args.limit
            .unwrap_or_else(|| ctx.out.default_limit().min(50))
            .max(1),
    );
    params.push_opt("offset", args.offset);

    let response = ctx.api.search(&params.build()).await?;
    emit(ctx, &response, query, &mode, args.offset.unwrap_or(0))
}

fn emit(
    ctx: &Ctx,
    response: &SearchResponse,
    query: &str,
    requested_mode: &str,
    offset: i64,
) -> CliResult<()> {
    let first_handle = (offset + 1) as usize;

    match ctx.out.format {
        Format::Json => ctx.out.json(response)?,
        Format::Yaml => ctx.out.yaml(response)?,
        Format::Ndjson => ctx.out.ndjson(&response.items)?,
        Format::Table | Format::Csv => {
            let grid = render::search_hits(ctx, &response.items, first_handle)?;
            ctx.out.grid(&grid)?;
        }
    }

    handles::save(
        HandleKind::Page,
        &format!("ovis search {query:?}"),
        response
            .items
            .iter()
            .enumerate()
            .map(|(index, hit)| HandleItem {
                n: first_handle + index,
                id: hit.document_id.clone(),
                label: hit
                    .semantic_id
                    .clone()
                    .unwrap_or_else(|| hit.document_id.clone()),
            })
            .collect(),
    );

    // The degradation notice belongs on stderr in every format, so a JSON
    // consumer that ignores the field still sees it, and stdout stays clean.
    if let Some(degraded) = &response.degraded {
        ctx.out
            .warn(format!("search degraded: {}", explain_degraded(degraded)));
        // The server echoes the *requested* mode and reports the fallback
        // separately, so "mode: hybrid" alone would read as though vectors ran.
        if requested_mode != "keyword" && degraded.starts_with("no_") {
            ctx.out.note(format!(
                "these are keyword results; {requested_mode} search could not run on this \
                 index"
            ));
        }
    }

    if ctx.out.format == Format::Table {
        let total = if response.total_hits_exact {
            thousands(response.total_hits)
        } else {
            format!("~{}", thousands(response.total_hits))
        };
        let mode_label = Tone::paint(
            if response.degraded.is_some() {
                Tone::Warn
            } else {
                Tone::Ok
            },
            &response.mode,
            ctx.out.color && ctx.out.stderr_tty,
        );
        ctx.out.footer(format!(
            "{} of {total} hits · mode {mode_label} · {} ms",
            response.items.len(),
            response.took_ms
        ));
        if !response.items.is_empty() {
            ctx.out
                .footer("view: ovis page view @N · text: ovis page text @N");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_live_degradation_is_explained_rather_than_shown_as_a_bare_token() {
        // The value this deployment actually returns.
        let explained = explain_degraded("no_knn_field");
        assert!(explained.starts_with("no_knn_field"));
        assert!(explained.contains("no document populates"), "{explained}");
        assert!(explained.contains("BM25"), "{explained}");
    }

    #[test]
    fn every_known_value_has_its_own_reading() {
        for value in [
            "no_knn_field",
            "no_embedder",
            "connector_filter_post_applied",
        ] {
            let explained = explain_degraded(value);
            assert!(explained.starts_with(value));
            assert!(
                explained.len() > value.len() + 10,
                "{value} needs an explanation, got {explained}"
            );
        }
    }

    #[test]
    fn an_unknown_degradation_is_surfaced_verbatim_rather_than_dropped() {
        // `degraded` is an open string; a future value must still reach the user.
        let explained = explain_degraded("some_future_reason");
        assert!(explained.starts_with("some_future_reason"), "{explained}");
    }
}
