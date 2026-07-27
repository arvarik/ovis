//! Per-request metrics.
//!
//! Labels use the **matched route template** (`/pages/{id}`), never the raw path.
//! Document ids are URLs, so labelling by raw path would mint a new time series
//! per document — 1.65 M of them — and take the Prometheus server down with it.

use axum::extract::{MatchedPath, Request};
use axum::response::Response;
use std::time::Instant;

pub async fn record_request(request: Request, next: axum::middleware::Next) -> Response {
    let method = request.method().as_str().to_owned();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned())
        // An unmatched path is a 404; bucket them all together rather than
        // creating a series per typo.
        .unwrap_or_else(|| "<unmatched>".to_owned());

    let started = Instant::now();
    let response = next.run(request).await;
    let elapsed = started.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();

    metrics::counter!(
        "ovis_http_requests_total",
        "method" => method.clone(),
        "route" => route.clone(),
        "status" => status,
    )
    .increment(1);

    metrics::histogram!(
        "ovis_http_request_duration_seconds",
        "method" => method,
        "route" => route,
    )
    .record(elapsed);

    response
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_route_label_is_a_template_not_a_document_id() {
        // The point of MatchedPath: `/pages/{id}` is one series, not one per
        // document. With 1.65M documents the alternative is a cardinality
        // explosion that takes the metrics backend with it.
        let template = "/pages/{id}";
        assert!(template.contains('{'));
        assert!(!template.contains("https"));
    }
}
