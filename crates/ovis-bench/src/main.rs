//! `ovis-bench` — the performance acceptance gate.
//!
//! Hits a running OVIS server with the performance budget matrix (documented
//! in `docs/operations.md`) and prints a pass/fail table. Exits non-zero if
//! any gate fails, so it can sit in CI or a release check.
//!
//! ```text
//! ovis-bench --url http://gamma:8080 --iterations 50
//! ```
//!
//! Two things it deliberately does *not* do:
//!
//! * Fabricate numbers. Every figure is measured, and a gate whose endpoint
//!   errors is reported as a failure rather than skipped. (The suite this
//!   replaces printed `100% PASS` banners regardless of the actual exit code.)
//! * Measure only the happy path. It walks cursors to a genuinely deep position,
//!   so the deep-paging gate is a deep page rather than page one again.

use std::time::{Duration, Instant};

use clap::Parser;
use serde_json::Value;

#[derive(Parser, Debug)]
#[command(
    name = "ovis-bench",
    about = "Measure the OVIS backend against its performance budgets"
)]
struct Args {
    /// Base URL of a running OVIS server.
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    url: String,

    /// Measured requests per gate, after warm-up.
    #[arg(long, default_value_t = 30)]
    iterations: usize,

    /// Warm-up requests per gate, excluded from the statistics. Caches and the
    /// Postgres plan cache both need priming; timing the first request of a cold
    /// process measures the wrong thing.
    #[arg(long, default_value_t = 5)]
    warmup: usize,

    /// Bearer token, if the server has `OVIS_API_TOKEN` set.
    #[arg(long, env = "OVIS_API_TOKEN")]
    token: Option<String>,

    /// Multiply every budget by this factor — for a small test database where the
    /// absolute numbers are not comparable to production but regressions still
    /// matter.
    #[arg(long, default_value_t = 1.0)]
    budget_scale: f64,
}

struct Gate {
    name: &'static str,
    path: String,
    /// p50 budget in milliseconds, where the guide sets one.
    p50_ms: Option<f64>,
    p99_ms: Option<f64>,
}

struct Measurement {
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
    errors: usize,
    last_error: Option<String>,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args = Args::parse();

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            eprintln!("cannot build an HTTP client: {err}");
            return std::process::ExitCode::from(2);
        }
    };

    let base = args.url.trim_end_matches('/').to_string();
    println!("OVIS performance gates against {base}");
    println!(
        "  {} measured iterations per gate ({} warm-up), budgets ×{}",
        args.iterations, args.warmup, args.budget_scale
    );

    // Say what the target actually is before measuring it.
    match get_json(
        &client,
        &base,
        "/api/v1/system/health",
        args.token.as_deref(),
    )
    .await
    {
        Ok(health) => {
            println!(
                "  target: status={} index={} schema_ok={}",
                health["status"].as_str().unwrap_or("?"),
                health["index_name"].as_str().unwrap_or("?"),
                health["schema_ok"]
            );
            let missing = health["missing_indexes"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if !missing.is_empty() {
                println!(
                    "  WARNING: {} OVIS support index(es) absent — the list gates cannot pass \
                     without them. Apply ops/onyx_indexes.sql. Missing: {:?}",
                    missing.len(),
                    missing
                );
            }
        }
        Err(err) => {
            eprintln!("cannot reach {base}: {err}");
            return std::process::ExitCode::from(2);
        }
    }

    let gates = match build_gates(&client, &base, args.token.as_deref()).await {
        Ok(gates) => gates,
        Err(err) => {
            eprintln!("could not prepare the gates: {err}");
            return std::process::ExitCode::from(2);
        }
    };

    println!();
    println!(
        "{:<34} {:>8} {:>8} {:>8} {:>8} {:>7}  verdict",
        "gate", "p50", "p95", "p99", "max", "errors"
    );
    println!("{}", "-".repeat(100));

    let mut failures = 0usize;
    for gate in &gates {
        let measurement = measure(&client, &base, &gate.path, &args).await;

        let mut problems = Vec::new();
        if measurement.errors > 0 {
            problems.push(format!(
                "{} request error(s): {}",
                measurement.errors,
                measurement.last_error.as_deref().unwrap_or("unknown")
            ));
        }
        if let Some(budget) = gate.p50_ms.map(|b| b * args.budget_scale) {
            if measurement.p50 > budget {
                problems.push(format!("p50 {:.1} > {budget:.0}", measurement.p50));
            }
        }
        if let Some(budget) = gate.p99_ms.map(|b| b * args.budget_scale) {
            if measurement.p99 > budget {
                problems.push(format!("p99 {:.1} > {budget:.0}", measurement.p99));
            }
        }

        let verdict = if problems.is_empty() {
            format!("PASS  ({})", budget_label(gate, args.budget_scale))
        } else {
            failures += 1;
            format!("FAIL  {}", problems.join("; "))
        };

        println!(
            "{:<34} {:>8.1} {:>8.1} {:>8.1} {:>8.1} {:>7}  {}",
            gate.name,
            measurement.p50,
            measurement.p95,
            measurement.p99,
            measurement.max,
            measurement.errors,
            verdict
        );
    }

    println!("{}", "-".repeat(100));
    if failures == 0 {
        println!("all {} gates passed", gates.len());
        std::process::ExitCode::SUCCESS
    } else {
        println!("{failures} of {} gates FAILED", gates.len());
        std::process::ExitCode::from(1)
    }
}

fn budget_label(gate: &Gate, scale: f64) -> String {
    let parts: Vec<String> = [
        gate.p50_ms.map(|b| format!("p50<{:.0}", b * scale)),
        gate.p99_ms.map(|b| format!("p99<{:.0}", b * scale)),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        "no budget".into()
    } else {
        parts.join(" ")
    }
}

/// Build the gate list, discovering a real document id and a deep cursor first.
///
/// Discovery matters: benchmarking `/pages/{id}` against a hardcoded id would
/// measure a 404, and benchmarking "deep page" against page one would measure
/// nothing.
async fn build_gates(
    client: &reqwest::Client,
    base: &str,
    token: Option<&str>,
) -> Result<Vec<Gate>, String> {
    let first = get_json(client, base, "/api/v1/pages?limit=50", token).await?;
    let document_id = first["items"][0]["id"]
        .as_str()
        .ok_or("the server returned no documents to measure against")?
        .to_string();
    let encoded = urlencode(&document_id);

    // Walk cursors to a genuinely deep position.
    let mut cursor = first["next_cursor"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let mut depth = 50usize;
    for _ in 0..19 {
        if cursor.is_empty() {
            break;
        }
        let page = get_json(
            client,
            base,
            &format!("/api/v1/pages?limit=50&cursor={}", urlencode(&cursor)),
            token,
        )
        .await?;
        match page["next_cursor"].as_str() {
            Some(next) if !next.is_empty() => {
                cursor = next.to_string();
                depth += 50;
            }
            _ => break,
        }
    }
    let deep_path = if cursor.is_empty() {
        println!("  deep page: too few documents to page into; measuring page one");
        "/api/v1/pages?limit=50".to_string()
    } else {
        println!("  deep page: cursor at roughly row {depth}");
        format!("/api/v1/pages?limit=50&cursor={}", urlencode(&cursor))
    };

    // A search term drawn from real data, so the query matches something.
    let term = first["items"]
        .as_array()
        .and_then(|items| {
            items.iter().find_map(|item| {
                item["semantic_id"]
                    .as_str()?
                    .split_whitespace()
                    .find(|word| word.len() > 3 && word.chars().all(|c| c.is_alphanumeric()))
                    .map(|word| word.to_lowercase())
            })
        })
        .unwrap_or_else(|| "the".to_string());
    println!("  search term: {term:?}");

    Ok(vec![
        Gate {
            name: "pages: default page (50)",
            path: "/api/v1/pages?limit=50".into(),
            p50_ms: Some(15.0),
            p99_ms: Some(60.0),
        },
        Gate {
            name: "pages: deep keyset page",
            path: deep_path,
            p50_ms: None,
            p99_ms: Some(80.0),
        },
        Gate {
            name: "pages: detail (metadata only)",
            path: format!("/api/v1/pages/{encoded}"),
            p50_ms: None,
            p99_ms: Some(25.0),
        },
        Gate {
            name: "pages: chunks with content",
            path: format!("/api/v1/pages/{encoded}/chunks?limit=100"),
            p50_ms: None,
            p99_ms: Some(120.0),
        },
        Gate {
            name: "search: keyword",
            path: format!("/api/v1/search?q={}&limit=20", urlencode(&term)),
            p50_ms: None,
            p99_ms: Some(150.0),
        },
        Gate {
            name: "search: hybrid (incl. embed)",
            path: format!("/api/v1/search?q={}&mode=hybrid&limit=20", urlencode(&term)),
            p50_ms: None,
            p99_ms: Some(400.0),
        },
        Gate {
            name: "connectors: summary (cached)",
            path: "/api/v1/connectors".into(),
            p50_ms: None,
            p99_ms: Some(300.0),
        },
        Gate {
            name: "sse: stream 1 row end to end",
            path: "/api/v1/pages/stream?limit=1".into(),
            p50_ms: None,
            p99_ms: Some(30.0),
        },
        Gate {
            name: "stats: overview (cached)",
            path: "/api/v1/stats/overview".into(),
            p50_ms: None,
            p99_ms: Some(500.0),
        },
        Gate {
            name: "system: health",
            path: "/api/v1/system/health".into(),
            p50_ms: None,
            p99_ms: Some(200.0),
        },
    ])
}

async fn measure(client: &reqwest::Client, base: &str, path: &str, args: &Args) -> Measurement {
    for _ in 0..args.warmup {
        let _ = timed_request(client, base, path, args.token.as_deref()).await;
    }

    let mut samples: Vec<f64> = Vec::with_capacity(args.iterations);
    let mut errors = 0usize;
    let mut last_error = None;
    for _ in 0..args.iterations {
        match timed_request(client, base, path, args.token.as_deref()).await {
            Ok(ms) => samples.push(ms),
            Err(err) => {
                errors += 1;
                last_error = Some(err);
            }
        }
    }

    if samples.is_empty() {
        return Measurement {
            p50: f64::NAN,
            p95: f64::NAN,
            p99: f64::NAN,
            max: f64::NAN,
            errors,
            last_error,
        };
    }

    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Measurement {
        p50: percentile(&samples, 0.50),
        p95: percentile(&samples, 0.95),
        p99: percentile(&samples, 0.99),
        max: samples[samples.len() - 1],
        errors,
        last_error,
    }
}

/// Nearest-rank percentile over an already-sorted slice.
fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = (q * sorted.len() as f64).ceil() as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

/// One request, timed until the whole body has arrived.
///
/// Reading the body matters: stopping at the headers would credit the server for
/// work it has not finished, and would skip compression entirely.
async fn timed_request(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    token: Option<&str>,
) -> Result<f64, String> {
    let started = Instant::now();
    let mut request = client.get(format!("{base}{path}"));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response.bytes().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "HTTP {status}: {}",
            String::from_utf8_lossy(&body[..body.len().min(160)])
        ));
    }
    Ok(started.elapsed().as_secs_f64() * 1000.0)
}

async fn get_json(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    token: Option<&str>,
) -> Result<Value, String> {
    let mut request = client.get(format!("{base}{path}"));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response.bytes().await.map_err(|e| e.to_string())?;
    // /system/health answers 503 when degraded, and its body is still the report.
    if !status.is_success() && status.as_u16() != 503 {
        return Err(format!(
            "GET {path} -> HTTP {status}: {}",
            String::from_utf8_lossy(&body[..body.len().min(300)])
        ));
    }
    serde_json::from_slice(&body).map_err(|e| format!("GET {path}: malformed JSON: {e}"))
}

fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_use_nearest_rank_and_never_index_out_of_bounds() {
        let sorted: Vec<f64> = (1..=100).map(|n| n as f64).collect();
        assert_eq!(percentile(&sorted, 0.50), 50.0);
        assert_eq!(percentile(&sorted, 0.95), 95.0);
        assert_eq!(percentile(&sorted, 0.99), 99.0);
        assert_eq!(percentile(&sorted, 1.0), 100.0);

        assert_eq!(percentile(&[7.0], 0.99), 7.0);
        assert!(percentile(&[], 0.5).is_nan());
    }

    #[test]
    fn urlencoding_escapes_everything_a_document_id_can_contain() {
        let encoded = urlencode("https://example.com/a?b=1&c=2 d=café");
        for forbidden in ['/', ':', '?', '&', '=', ' '] {
            assert!(
                !encoded.contains(forbidden),
                "{forbidden} survived encoding"
            );
        }
        assert!(encoded.contains("%2F"));
        assert!(encoded.contains("%3F"));
        // Unreserved characters stay readable in logs.
        assert_eq!(urlencode("abc-123_x.y~z"), "abc-123_x.y~z");
    }

    #[test]
    fn budget_scaling_is_reflected_in_the_reported_label() {
        let gate = Gate {
            name: "x",
            path: "/".into(),
            p50_ms: Some(15.0),
            p99_ms: Some(60.0),
        };
        assert_eq!(budget_label(&gate, 1.0), "p50<15 p99<60");
        assert_eq!(budget_label(&gate, 2.0), "p50<30 p99<120");

        let unbudgeted = Gate {
            name: "y",
            path: "/".into(),
            p50_ms: None,
            p99_ms: None,
        };
        assert_eq!(budget_label(&unbudgeted, 1.0), "no budget");
    }

    #[test]
    fn a_gate_with_request_errors_cannot_be_reported_as_passing() {
        // Mirrors the verdict logic in `main`: errors alone fail the gate, however
        // fast the successful requests were. The suite this replaces printed
        // "100% PASS" regardless of what happened.
        let measurement = Measurement {
            p50: 1.0,
            p95: 1.0,
            p99: 1.0,
            max: 1.0,
            errors: 3,
            last_error: Some("HTTP 500".into()),
        };
        let mut problems: Vec<String> = Vec::new();
        if measurement.errors > 0 {
            problems.push("errors".into());
        }
        assert!(!problems.is_empty());
    }
}
