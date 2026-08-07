//! Binary-level tests: the actual `ovis` executable, against a mock server.
//!
//! These cover what unit tests cannot — exit codes, the stdout/stderr split, and
//! the fact that a failure is a failure. `tests/cli_tests.rs` previously never
//! executed the binary at all; it unit-tested `try_parse_from` and called it
//! coverage.

use std::process::Output;

use assert_cmd::prelude::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A run of the binary in an isolated config/state directory, so tests never
/// read the developer's real `~/.config/ovis` or clobber their `@N` handles.
struct Run {
    home: tempfile::TempDir,
    server: String,
}

impl Run {
    fn new(server: &str) -> Self {
        Self {
            home: tempfile::tempdir().expect("tempdir"),
            server: server.to_string(),
        }
    }

    fn cmd(&self, args: &[&str]) -> Output {
        self.build(args)
            .env("OVIS_SERVER", &self.server)
            .output()
            .expect("the binary runs")
    }

    /// Without `OVIS_SERVER`, so config-file and profile resolution is what
    /// decides the server — the environment would otherwise (correctly) win.
    fn cmd_no_env_server(&self, args: &[&str]) -> Output {
        self.build(args).output().expect("the binary runs")
    }

    fn build(&self, args: &[&str]) -> std::process::Command {
        let mut command = std::process::Command::cargo_bin("ovis").expect("the binary builds");
        command
            .args(args)
            .env("OVIS_CONFIG", self.home.path().join("config.toml"))
            .env("XDG_STATE_HOME", self.home.path().join("state"))
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env_remove("OVIS_SERVER")
            .env_remove("OVIS_TOKEN")
            .env_remove("OVIS_PROFILE")
            .env("NO_COLOR", "1");
        command
    }
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn envelope(code: &str, message: &str, status: u16) -> serde_json::Value {
    serde_json::json!({
        "error": { "code": code, "message": message, "status": status, "req_id": "01JTEST" }
    })
}

fn page_list(items: serde_json::Value, total: i64) -> serde_json::Value {
    serde_json::json!({
        "items": items,
        "total": total,
        "total_exact": true,
        "page": 1,
        "limit": 50,
        "next_cursor": null,
        "has_more": false
    })
}

fn one_page() -> serde_json::Value {
    serde_json::json!({
        "id": "https://example.com/a",
        "semantic_id": "A page",
        "link": "https://example.com/a",
        "updated_at": "2026-07-20T00:00:00Z",
        "doc_updated_at": null,
        "last_modified": "2026-07-20T00:00:00Z",
        "chunk_count": 3,
        "boost": 0,
        "hidden": false,
        "connector_id": 4,
        "connector_name": "tildes",
        "connector_source": "WEB",
        "metadata": null
    })
}

// ---------------------------------------------------------------------------
// No server at all
// ---------------------------------------------------------------------------

#[test]
fn an_unreachable_server_is_exit_12_on_every_verb_and_never_sample_data() {
    // Port 1 is never listening.
    let run = Run::new("http://127.0.0.1:1");
    for args in [
        vec!["page", "list"],
        vec!["connector", "list"],
        vec!["search", "kant"],
        vec!["stats"],
        vec!["status"],
    ] {
        let output = run.cmd(&args);
        assert_eq!(code(&output), 12, "{args:?} should exit 12");
        assert!(
            stdout(&output).trim().is_empty(),
            "{args:?} wrote data despite failing: {:?}",
            stdout(&output)
        );
        assert!(
            stderr(&output).contains("cannot reach OVIS server"),
            "{args:?}"
        );
        assert!(stderr(&output).contains("hint:"), "{args:?} gave no hint");
    }
}

#[test]
fn an_unreachable_server_makes_json_fail_rather_than_emitting_fabricated_items() {
    // The exact defect: the old CLI answered `--format json` with five baked-in
    // sample documents and exit 0.
    let run = Run::new("http://127.0.0.1:1");
    let output = run.cmd(&["page", "list", "--format", "json"]);
    assert_eq!(code(&output), 12);
    assert!(stdout(&output).is_empty());
    assert!(!stdout(&output).contains("docs.onyx.app"));
}

#[test]
fn deleting_with_the_server_down_reports_failure_rather_than_success() {
    let run = Run::new("http://127.0.0.1:1");
    let output = run.cmd(&["page", "delete", "https://example.com/a", "-y"]);
    assert_eq!(code(&output), 12);
    assert!(!stdout(&output).contains("deleted"));
    assert!(stderr(&output).contains("cannot reach OVIS server"));
}

// ---------------------------------------------------------------------------
// Against a mock server
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn json_output_is_the_wire_struct_and_nothing_else_on_stdout() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/pages"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(page_list(serde_json::json!([one_page()]), 1)),
        )
        .mount(&server)
        .await;

    let run = Run::new(&server.uri());
    // The flag comes *after* the subcommand — the headline parsing defect.
    let output = run.cmd(&["page", "list", "--format", "json"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));

    let parsed: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("stdout is pure JSON with no extra bytes");
    assert_eq!(parsed["total"], 1);
    assert_eq!(parsed["items"][0]["id"], "https://example.com/a");
    // Diagnostics went to stderr, where they cannot corrupt a pipeline.
    assert!(!stdout(&output).contains("info:"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_error_envelope_maps_to_its_documented_exit_code() {
    for (code_name, status, expected_exit) in [
        ("NOT_FOUND", 404u16, 3),
        ("BAD_REQUEST", 400, 2),
        ("DATABASE", 500, 1),
        ("UNAUTHORIZED", 401, 1),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/pages"))
            .respond_with(ResponseTemplate::new(status).set_body_json(envelope(
                code_name,
                "something went wrong",
                status,
            )))
            .mount(&server)
            .await;

        let run = Run::new(&server.uri());
        let output = run.cmd(&["page", "list"]);
        assert_eq!(
            code(&output),
            expected_exit,
            "{code_name} should exit {expected_exit}: {}",
            stderr(&output)
        );
        // The code and request id travel with the message, so a "database
        // error" can be traced to its log line.
        assert!(stderr(&output).contains(code_name), "{code_name}");
        assert!(stderr(&output).contains("01JTEST"), "{code_name}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unauthorized_response_says_how_to_supply_a_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/pages"))
        .respond_with(ResponseTemplate::new(401).set_body_json(envelope(
            "UNAUTHORIZED",
            "no token",
            401,
        )))
        .mount(&server)
        .await;

    let output = Run::new(&server.uri()).cmd(&["page", "list"]);
    assert!(stderr(&output).contains("--token"), "{}", stderr(&output));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_single_delete_reports_what_actually_happened() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/pages/https%3A%2F%2Fexample.com%2Fa"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "https://example.com/a",
            "semantic_id": "A page",
            "link": "https://example.com/a",
            "updated_at": "2026-07-20T00:00:00Z",
            "doc_updated_at": null,
            "last_modified": "2026-07-20T00:00:00Z",
            "chunk_count": 3,
            "boost": 0,
            "hidden": false,
            "connector_id": 4,
            "connector_name": "tildes",
            "connector_source": "WEB",
            "metadata": null,
            "primary_owners": null,
            "secondary_owners": null,
            "content_hash": null,
            "from_ingestion_api": false,
            "last_synced": null,
            "cc_pair_id": 5,
            "cc_pair_status": "ACTIVE",
            "tags": [],
            "pg_row": true,
            "recrawl_risk": true
        })))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/pages/https%3A%2F%2Fexample.com%2Fa"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "pg_deleted": true,
            "chunks_deleted": 3,
            "index_cleanup_pending": true,
            "recrawl_risk": true
        })))
        .mount(&server)
        .await;

    let output = Run::new(&server.uri()).cmd(&["page", "delete", "https://example.com/a", "-y"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("3 chunks removed"),
        "{}",
        stdout(&output)
    );
    // Both honest-outcome fields are surfaced rather than being papered over
    // with "success".
    assert!(
        stderr(&output).contains("pending_index_deletes"),
        "{}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("crawl it again"),
        "{}",
        stderr(&output)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_partly_failed_batch_delete_exits_11_and_names_the_failures() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404).set_body_json(envelope("NOT_FOUND", "no", 404)))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/pages/batch-delete"))
        .respond_with(ResponseTemplate::new(207).set_body_json(serde_json::json!({
            "success": false,
            "deleted": 2,
            "chunks_deleted": 12,
            "failed": [{ "id": "https://example.com/c", "code": "OPENSEARCH_UPSTREAM" }],
            "index_cleanup_pending": 0
        })))
        .mount(&server)
        .await;

    let output = Run::new(&server.uri()).cmd(&[
        "page",
        "delete",
        "-y",
        "https://example.com/a",
        "https://example.com/b",
        "https://example.com/c",
    ]);
    assert_eq!(code(&output), 11, "{}", stderr(&output));
    assert!(stdout(&output).contains("deleted 2"), "{}", stdout(&output));
    assert!(
        stderr(&output).contains("OPENSEARCH_UPSTREAM"),
        "{}",
        stderr(&output)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_input_refuses_to_delete_and_never_reaches_the_server() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404).set_body_json(envelope("NOT_FOUND", "no", 404)))
        .mount(&server)
        .await;
    // No DELETE mock at all: if the CLI issued one, wiremock would answer 404
    // and the exit code would not be 10.

    let output =
        Run::new(&server.uri()).cmd(&["page", "delete", "https://example.com/a", "--no-input"]);
    assert_eq!(code(&output), 10, "{}", stderr(&output));
    assert!(stderr(&output).contains("--no-input"));
    assert!(server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|r| r.method != http_types_method_delete()));
}

fn http_types_method_delete() -> wiremock::http::Method {
    wiremock::http::Method::DELETE
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_degraded_server_exits_13_and_still_prints_the_panel() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/system/health"))
        .respond_with(ResponseTemplate::new(503).set_body_json(serde_json::json!({
            "status": "degraded",
            "postgres": { "status": "down", "latency_ms": null, "detail": "connection refused" },
            "opensearch": { "status": "ok", "latency_ms": 3.0, "detail": null },
            "onyx_api": { "configured": false, "status": "not_configured", "latency_ms": null,
                          "version": null, "detail": null },
            "embedder": { "status": "ok", "latency_ms": 4.0, "detail": null },
            "schema_ok": true,
            "missing_columns": [],
            "unhandled_document_fk_children": [],
            "missing_indexes": [],
            "index_name": "danswer_chunk_snowflake_arctic_embed_m",
            "version": "0.2.0"
        })))
        .mount(&server)
        .await;

    let output = Run::new(&server.uri()).cmd(&["status"]);
    assert_eq!(code(&output), 13, "{}", stderr(&output));
    // The panel is still data: a degraded server is an answer, not a failure to
    // produce one.
    assert!(stdout(&output).contains("postgres"), "{}", stdout(&output));
    assert!(
        stderr(&output).contains("postgres down"),
        "{}",
        stderr(&output)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_surfaces_the_degraded_value_rather_than_looking_broken() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [],
            "mode": "hybrid",
            "degraded": "no_knn_field",
            "total_hits": 0,
            "total_hits_exact": true,
            "took_ms": 12
        })))
        .mount(&server)
        .await;

    let output = Run::new(&server.uri()).cmd(&["search", "kant", "--mode", "hybrid"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("no_knn_field"),
        "{}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("keyword"),
        "the fallback has to be spelled out: {}",
        stderr(&output)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handles_from_a_list_resolve_on_the_next_command() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/pages"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(page_list(serde_json::json!([one_page()]), 1)),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/pages/https%3A%2F%2Fexample.com%2Fa"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "https://example.com/a", "semantic_id": "A page",
            "link": "https://example.com/a", "updated_at": "2026-07-20T00:00:00Z",
            "doc_updated_at": null, "last_modified": "2026-07-20T00:00:00Z",
            "chunk_count": 3, "boost": 0, "hidden": false, "connector_id": 4,
            "connector_name": "tildes", "connector_source": "WEB", "metadata": null,
            "primary_owners": null, "secondary_owners": null, "content_hash": null,
            "from_ingestion_api": false, "last_synced": null, "cc_pair_id": 5,
            "cc_pair_status": "PAUSED", "tags": [], "pg_row": true, "recrawl_risk": false
        })))
        .mount(&server)
        .await;

    let run = Run::new(&server.uri());
    assert_eq!(code(&run.cmd(&["page", "list"])), 0);

    let output = run.cmd(&["page", "view", "@1"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("A page"));
}

#[test]
fn an_unknown_handle_exits_14_and_explains_the_freshness_rule() {
    let run = Run::new("http://127.0.0.1:1");
    let output = run.cmd(&["page", "view", "@7"]);
    assert_eq!(code(&output), 14, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("expire after an hour"),
        "{}",
        stderr(&output)
    );
}

// ---------------------------------------------------------------------------
// Parsing, help, and the retired surface
// ---------------------------------------------------------------------------

#[test]
fn help_and_version_exit_zero_and_a_bad_flag_exits_two() {
    let run = Run::new("http://127.0.0.1:1");
    assert_eq!(code(&run.cmd(&["--help"])), 0);
    assert_eq!(code(&run.cmd(&["--version"])), 0);
    assert_eq!(code(&run.cmd(&["page", "list", "--help"])), 0);
    assert_eq!(code(&run.cmd(&["--nonsense"])), 2);
    assert_eq!(code(&run.cmd(&["page", "list", "--nonsense"])), 2);
    assert_eq!(code(&run.cmd(&["nonsense"])), 2);
}

#[test]
fn the_removed_credential_flags_no_longer_parse() {
    let run = Run::new("http://127.0.0.1:1");
    for flag in [
        "--db-dsn",
        "--postgres-url",
        "--opensearch-url",
        "--search-engine",
    ] {
        let output = run.cmd(&["page", "list", flag, "x"]);
        assert_eq!(code(&output), 2, "{flag} still parses");
    }
}

#[test]
fn no_compiled_in_credential_survives_in_the_binary() {
    // The old build carried `postgres://postgres:<password>@192.168.4.113:5433`
    // in its rodata, printed it to stdout, and passed it in argv where `ps`
    // could read it. The homelab *host* still appears — in the config
    // template's example profile and in the backend's "OPENSEARCH_URL is
    // required (e.g. …)" message — which is documentation, not a secret. A
    // DSN carrying a password is the thing that must not be there.
    let binary = std::process::Command::cargo_bin("ovis").expect("the binary builds");
    let bytes = std::fs::read(binary.get_program()).expect("the binary is readable");
    let haystack = String::from_utf8_lossy(&bytes);

    for line in haystack.split(|c: char| c.is_control()) {
        if let Some(rest) = line.split("postgres://").nth(1) {
            // `user:password@host` — an `@` before the next slash with a colon
            // in front of it is a credential.
            let authority = rest.split('/').next().unwrap_or("");
            if let Some(userinfo) = authority
                .split('@')
                .next()
                .filter(|_| authority.contains('@'))
            {
                assert!(
                    !userinfo.contains(':') || userinfo.ends_with(':') || userinfo.contains('…'),
                    "the binary embeds a DSN with a password: {line}"
                );
            }
        }
    }
    assert!(!haystack.contains("ONYX_ADMIN_PASSWORD"));
}

#[test]
fn prune_requires_a_subcommand_and_a_scan_scope() {
    let run = Run::new("http://127.0.0.1:1");
    // Bare `ovis prune` shows the verb list (the house convention: help on a
    // missing subcommand is exit 0), and it is a real command tree now.
    let output = run.cmd(&["prune"]);
    assert_eq!(code(&output), 0);
    // clap routes missing-subcommand help through the error path: stderr.
    let help = format!("{}{}", stdout(&output), stderr(&output));
    for verb in ["scan", "staged", "restore", "delete", "status", "log"] {
        assert!(help.contains(verb), "missing verb {verb}: {help}");
    }
    assert!(
        !help.contains("deferred"),
        "the deferred stub must be gone: {help}"
    );

    // A scan must say what to scan; detectors are mandatory.
    let output = run.cmd(&["prune", "scan", "-d", "thin"]);
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(stderr(&output).contains("--all"), "{}", stderr(&output));
}

#[test]
fn prune_delete_has_no_now_flag_by_grammar() {
    // The lifecycle's core promise: nothing deletes inline. `--now` must not
    // even parse.
    let run = Run::new("http://127.0.0.1:1");
    let output = run.cmd(&["prune", "delete", "@1", "--now"]);
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(stderr(&output).contains("--now"), "{}", stderr(&output));
}

#[test]
fn prune_status_reports_an_unreachable_server_as_exit_12() {
    let run = Run::new("http://127.0.0.1:1");
    let output = run.cmd(&["prune", "status"]);
    assert_eq!(code(&output), 12, "{}", stderr(&output));
}

#[test]
fn prune_stage_refuses_without_ids_or_filter_before_any_request() {
    // Usage validation happens before the network: the server here is
    // unreachable, so exit 2 (not 12) proves the order.
    let run = Run::new("http://127.0.0.1:1");
    let output = run.cmd(&["prune", "stage"]);
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(stderr(&output).contains("--filter"), "{}", stderr(&output));
}

#[test]
fn an_unknown_column_is_a_usage_error_naming_the_valid_ones() {
    let run = Run::new("http://127.0.0.1:1");
    let output = run.cmd(&["page", "list", "--columns", "titel"]);
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(stderr(&output).contains("title"), "{}", stderr(&output));
}

#[test]
fn a_malformed_time_filter_fails_before_any_request_is_made() {
    let run = Run::new("http://127.0.0.1:1");
    let output = run.cmd(&["page", "list", "--since", "last tuesday"]);
    // Exit 2, not 12: the input was rejected without ever contacting a server.
    assert_eq!(code(&output), 2, "{}", stderr(&output));
}

#[test]
fn yaml_streaming_is_refused_with_an_alternative_rather_than_buffering() {
    let run = Run::new("http://127.0.0.1:1");
    let output = run.cmd(&["page", "list", "--all", "-o", "yaml"]);
    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("ndjson"), "{}", stderr(&output));
}

#[test]
fn config_init_writes_a_file_that_config_show_can_read_back() {
    let run = Run::new("http://127.0.0.1:1");
    assert_eq!(code(&run.cmd(&["config", "init"])), 0);
    // A second init without --force must not silently overwrite.
    assert_eq!(code(&run.cmd(&["config", "init"])), 2);

    let output = run.cmd(&["config", "show", "--origin"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("server"));

    assert_eq!(
        code(&run.cmd(&["config", "set", "profiles.p.server", "http://gamma:8080"])),
        0
    );
    // No OVIS_SERVER in the environment, so the profile is what decides.
    let output = run.cmd_no_env_server(&["--profile", "p", "config", "show"]);
    assert!(
        stdout(&output).contains("http://gamma:8080"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn the_environment_beats_the_profile_and_a_flag_beats_the_environment() {
    let run = Run::new("http://127.0.0.1:1");
    assert_eq!(code(&run.cmd(&["config", "init"])), 0);
    assert_eq!(
        code(&run.cmd(&["config", "set", "profiles.p.server", "http://profile:1"])),
        0
    );

    let from_profile = run.cmd_no_env_server(&["--profile", "p", "config", "show", "--origin"]);
    assert!(stdout(&from_profile).contains("http://profile:1"));

    // OVIS_SERVER is set by `cmd`, and must win.
    let from_env = run.cmd(&["--profile", "p", "config", "show", "--origin"]);
    assert!(
        stdout(&from_env).contains("http://127.0.0.1:1"),
        "{}",
        stdout(&from_env)
    );
    assert!(
        stdout(&from_env).contains("env OVIS_SERVER"),
        "{}",
        stdout(&from_env)
    );

    // …and --server wins over that.
    let from_flag = run.cmd(&[
        "--profile",
        "p",
        "--server",
        "http://flag:1",
        "config",
        "show",
        "--origin",
    ]);
    assert!(
        stdout(&from_flag).contains("http://flag:1"),
        "{}",
        stdout(&from_flag)
    );
    assert!(
        stdout(&from_flag).contains("flag"),
        "{}",
        stdout(&from_flag)
    );
}

#[test]
fn config_show_never_prints_a_token() {
    let run = Run::new("http://127.0.0.1:1");
    assert_eq!(code(&run.cmd(&["config", "init"])), 0);
    assert_eq!(
        code(&run.cmd(&["config", "set", "profiles.p.token", "super-secret"])),
        0
    );
    for args in [
        vec!["config", "show"],
        vec!["config", "show", "-o", "json"],
        vec!["config", "show", "-o", "yaml"],
    ] {
        let output = run.cmd(&args);
        assert!(
            !stdout(&output).contains("super-secret"),
            "{args:?} leaked the token: {}",
            stdout(&output)
        );
    }
}

#[test]
fn an_unknown_profile_fails_loudly_instead_of_silently_using_the_default_server() {
    let run = Run::new("http://127.0.0.1:1");
    assert_eq!(code(&run.cmd(&["config", "init"])), 0);
    let output = run.cmd(&["--profile", "nope", "status"]);
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(stderr(&output).contains("nope"));
}

#[test]
fn completions_are_generated_for_every_documented_shell() {
    let run = Run::new("http://127.0.0.1:1");
    for shell in ["bash", "zsh", "fish"] {
        let output = run.cmd(&["completions", shell]);
        assert_eq!(code(&output), 0, "{shell}: {}", stderr(&output));
        assert!(
            stdout(&output).contains("ovis __connector-names"),
            "{shell}"
        );
        assert!(stderr(&output).contains("install:"), "{shell}");
    }
}

// ---------------------------------------------------------------------------
// The server noun
// ---------------------------------------------------------------------------

#[test]
fn a_detached_start_that_dies_reports_the_failure_instead_of_claiming_success() {
    // It used to spawn, write a PID file, print "started in the background" and
    // exit 0 — for a process that was already gone. The reason is quoted from
    // the log the child now writes to.
    let run = Run::new("http://127.0.0.1:1");
    let output = run
        .build(&["server", "start", "-d", "--port", "8079"])
        .env("DATABASE_URL", "")
        .env("OPENSEARCH_URL", "")
        .output()
        .expect("the binary runs");

    assert_ne!(code(&output), 0, "a dead server must not report success");
    assert!(
        stderr(&output).contains("exited immediately"),
        "{}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("DATABASE_URL is required"),
        "the child's own reason should be quoted: {}",
        stderr(&output)
    );
    // …and no PID file was left behind to block the next `-d`.
    assert!(!run.home.path().join("state/ovis/server.pid").exists());
}

#[test]
fn a_detached_start_does_not_hold_the_parents_stdout_open() {
    // The child logs to a file rather than inheriting our stdout: an inherited
    // pipe stays open for the life of the server, so `ovis server start -d |
    // grep …` would hang forever. This asserts the run *terminates*; whether it
    // succeeds depends on there being a database, which there is not here.
    let run = Run::new("http://127.0.0.1:1");
    let started = std::time::Instant::now();
    let output = run
        .build(&["server", "start", "-d", "--port", "8078"])
        .env("DATABASE_URL", "")
        .env("OPENSEARCH_URL", "")
        .output()
        .expect("the binary runs");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "the detached start did not return"
    );
    assert_ne!(code(&output), 0);
}

#[test]
fn the_backend_never_reads_the_config_file_the_cli_owns() {
    // Both programs honour `OVIS_CONFIG`, and they mean different files by it:
    // to the CLI it is the profiles/ui/tui file, to the backend a flat
    // ServerConfig table. `ovis server start` used to let the backend's loader
    // fall through to it and die with "config file does not exist".
    let run = Run::new("http://127.0.0.1:1");
    let output = run
        .build(&["server", "start", "-d", "--port", "8077"])
        .env("DATABASE_URL", "")
        .env("OPENSEARCH_URL", "")
        .output()
        .expect("the binary runs");

    assert!(
        !stderr(&output).contains("does not exist"),
        "the backend read the CLI's config path: {}",
        stderr(&output)
    );
    // It failed for the right reason instead.
    assert!(
        stderr(&output).contains("DATABASE_URL is required"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn stopping_when_nothing_is_running_is_a_clean_no_op() {
    let run = Run::new("http://127.0.0.1:1");
    let output = run.cmd(&["server", "stop"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stderr(&output).contains("no background server"));
}

#[test]
fn the_tui_refuses_to_run_without_a_terminal_rather_than_garbling_a_pipe() {
    let run = Run::new("http://127.0.0.1:1");
    let output = run.cmd(&["tui"]);
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(stderr(&output).contains("needs a terminal"));
}
