//! Direct CLI subcommand coverage.
//!
//! Every leaf subcommand has at least one test here. The pattern is:
//!   1. spawn a wiremock CP that returns the documented response shape
//!      for whatever HTTP endpoint the subcommand calls
//!   2. invoke the real `nixfleet` binary via assert_cmd
//!   3. assert exit code + relevant stdout/stderr/side effects
//!
//! Subcommands that don't talk to a CP (host add, init) chdir into a
//! tempdir and assert the generated files / config.
//!
//! Subcommands covered by dedicated scenario files are NOT duplicated
//! here:
//!   - deploy — `vm-fleet`, `vm-fleet-bootstrap`
//!   - rollback (both `--ssh` and refusal paths) — `rollback_cli_scenarios.rs`
//!   - rollout resume — `vm-fleet-apply-failure`
//!   - release create (+ push hook) — `vm-fleet-release`, `release_hook_scenarios.rs`
//!   - release delete — `release_delete_scenarios.rs`

use assert_cmd::Command;
use predicates::prelude::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Build a MockServer with the auth endpoint pre-mocked. Every test
/// here uses `--api-key test-key` so the CP would normally check it,
/// but the mock doesn't validate the bearer — it only matches by
/// method+path. The tests assert CLI behavior, not CP auth.
async fn cp_mock() -> MockServer {
    MockServer::start().await
}

// =====================================================================
// init — local file generation, no CP
// =====================================================================

#[test]
fn init_writes_config_file_in_cwd() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("nixfleet")
        .unwrap()
        .current_dir(dir.path())
        .args([
            "init",
            "--control-plane-url",
            "https://lab.example.com:8080",
            "--ca-cert",
            "/run/secrets/fleet-ca.pem",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(".nixfleet.toml"));

    let config_path = dir.path().join(".nixfleet.toml");
    assert!(config_path.exists(), "init must write .nixfleet.toml in cwd");
    let body = std::fs::read_to_string(&config_path).unwrap();
    assert!(body.contains("https://lab.example.com:8080"));
    assert!(body.contains("/run/secrets/fleet-ca.pem"));
}

// =====================================================================
// host add — local file generation under modules/_hardware/<host>/
// =====================================================================

#[test]
fn host_add_generates_disk_config_and_prints_snippet() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("nixfleet")
        .unwrap()
        .current_dir(dir.path())
        .args([
            "host",
            "add",
            "--hostname",
            "edge-42",
            "--org",
            "test-org",
            "--role",
            "edge",
            "--platform",
            "x86_64-linux",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("edge-42"))
        // The fleet.nix snippet block is printed to stdout.
        .stdout(predicate::str::contains("mkHost"));

    let disk_config = dir
        .path()
        .join("modules/_hardware/edge-42/disk-config.nix");
    assert!(
        disk_config.exists(),
        "host add must generate modules/_hardware/<host>/disk-config.nix"
    );
    let body = std::fs::read_to_string(&disk_config).unwrap();
    assert!(body.contains("disko.devices.disk.main"));
}

// =====================================================================
// bootstrap — POST /api/v1/keys/bootstrap
// =====================================================================

#[tokio::test]
async fn bootstrap_prints_key_on_success() {
    let server = cp_mock().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/keys/bootstrap"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "key": "nfk-fake-admin-key",
            "name": "admin",
            "role": "admin"
        })))
        .mount(&server)
        .await;

    Command::cargo_bin("nixfleet")
        .unwrap()
        .args([
            "--control-plane-url",
            &server.uri(),
            "bootstrap",
            "--name",
            "admin",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("nfk-fake-admin-key"));
}

#[tokio::test]
async fn bootstrap_fails_on_409_keys_exist() {
    let server = cp_mock().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/keys/bootstrap"))
        .respond_with(ResponseTemplate::new(409))
        .mount(&server)
        .await;

    Command::cargo_bin("nixfleet")
        .unwrap()
        .args([
            "--control-plane-url",
            &server.uri(),
            "bootstrap",
            "--name",
            "admin",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("API keys already exist"));
}

// =====================================================================
// status — GET /api/v1/machines (status reads the machine list)
// =====================================================================

/// MachineStatus DTO matches `shared/src/lib.rs::MachineStatus`. The
/// real type rejects null for required string fields, so this helper
/// builds a minimal valid value with empty strings instead of nulls.
fn fake_machine_status_json(id: &str, tags: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "machine_id": id,
        "current_generation": "",
        "desired_generation": null,
        "agent_version": "",
        "system_state": "",
        "uptime_seconds": 0,
        "last_report": null,
        "lifecycle": "active",
        "tags": tags
    })
}

#[tokio::test]
async fn status_lists_machines() {
    let server = cp_mock().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/machines"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            fake_machine_status_json("web-01", &["web"]),
            fake_machine_status_json("db-01", &["db"]),
        ])))
        .mount(&server)
        .await;

    Command::cargo_bin("nixfleet")
        .unwrap()
        .args([
            "--control-plane-url",
            &server.uri(),
            "--api-key",
            "test-key",
            "status",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("web-01"))
        .stdout(predicate::str::contains("db-01"));
}

// =====================================================================
// machines list — GET /api/v1/machines (with and without --tags)
// =====================================================================

#[tokio::test]
async fn machines_list_no_filter_returns_all() {
    let server = cp_mock().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/machines"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            fake_machine_status_json("web-01", &["web"]),
        ])))
        .mount(&server)
        .await;

    Command::cargo_bin("nixfleet")
        .unwrap()
        .args([
            "--control-plane-url",
            &server.uri(),
            "--api-key",
            "test-key",
            "machines",
            "list",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("web-01"));
}

#[tokio::test]
async fn machines_list_filters_client_side_by_tag() {
    let server = cp_mock().await;

    // The CLI fetches all machines and filters client-side (it does
    // NOT pass tag as a query param). This test asserts that
    // --tags web shows web-01 but excludes db-01.
    Mock::given(method("GET"))
        .and(path("/api/v1/machines"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            fake_machine_status_json("web-01", &["web"]),
            fake_machine_status_json("db-01", &["db"]),
        ])))
        .mount(&server)
        .await;

    let assert = Command::cargo_bin("nixfleet")
        .unwrap()
        .args([
            "--control-plane-url",
            &server.uri(),
            "--api-key",
            "test-key",
            "machines",
            "list",
            "--tags",
            "web",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("web-01"), "expected web-01 in output: {stdout}");
    assert!(
        !stdout.contains("db-01"),
        "tag filter must exclude db-01 from output: {stdout}"
    );
}

// =====================================================================
// machines list --json — verify output is valid JSON
// =====================================================================

#[tokio::test]
async fn machines_list_json_output_is_valid() {
    let server = cp_mock().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/machines"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "machine_id": "web-01",
                    "current_generation": "/nix/store/abc-system",
                    "desired_generation": null,
                    "agent_version": "",
                    "system_state": "ok",
                    "uptime_seconds": 0,
                    "last_report": null,
                    "lifecycle": "active",
                    "tags": ["web"]
                }
            ])),
        )
        .mount(&server)
        .await;

    let assert = Command::cargo_bin("nixfleet")
        .unwrap()
        .args([
            "--control-plane-url",
            &server.uri(),
            "--api-key",
            "test-key",
            "--json",
            "machines",
            "list",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    // Verify it's valid JSON
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|_| panic!("expected valid JSON, got: {stdout}"));
    assert!(parsed.is_array(), "expected JSON array, got: {parsed}");
}

// =====================================================================
// machines untag — DELETE /api/v1/machines/{id}/tags/{tag}
// =====================================================================

#[tokio::test]
async fn machines_untag_calls_delete_endpoint() {
    let server = cp_mock().await;

    Mock::given(method("DELETE"))
        .and(path("/api/v1/machines/web-01/tags/web"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    Command::cargo_bin("nixfleet")
        .unwrap()
        .args([
            "--control-plane-url",
            &server.uri(),
            "--api-key",
            "test-key",
            "machines",
            "untag",
            "web-01",
            "web",
        ])
        .assert()
        .success();

    // Verify the mock actually received the DELETE — proves the CLI
    // built the URL correctly.
    let received = server.received_requests().await.unwrap();
    assert!(
        received
            .iter()
            .any(|r| r.method.as_str() == "DELETE"
                && r.url.path() == "/api/v1/machines/web-01/tags/web"),
        "expected DELETE /api/v1/machines/web-01/tags/web in mock requests"
    );
}

// =====================================================================
// machines register — POST /api/v1/machines/{id}/register
// =====================================================================

#[tokio::test]
async fn machines_register_posts_with_tags() {
    let server = cp_mock().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/machines/edge-99/register"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;

    Command::cargo_bin("nixfleet")
        .unwrap()
        .args([
            "--control-plane-url",
            &server.uri(),
            "--api-key",
            "test-key",
            "machines",
            "register",
            "edge-99",
            "--tags",
            "edge",
            "--tags",
            "us-west",
        ])
        .assert()
        .success();

    let received = server.received_requests().await.unwrap();
    let req = received
        .iter()
        .find(|r| r.url.path() == "/api/v1/machines/edge-99/register")
        .expect("register request must reach the mock");
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    let tags = body
        .get("tags")
        .and_then(|t| t.as_array())
        .expect("body must have tags array");
    assert!(tags.iter().any(|t| t == "edge"));
    assert!(tags.iter().any(|t| t == "us-west"));
}

// =====================================================================
// rollout list — GET /api/v1/rollouts (with ?status= query filter)
// =====================================================================
//
// The plain `rollout list` routing case is dispatched transitively by
// every other rollout CLI test (they all go through the same clap
// enum). The only thing worth pinning explicitly is the query-param
// builder, since that's CLI logic with no other coverage.

#[tokio::test]
async fn rollout_list_with_status_filter() {
    let server = cp_mock().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/rollouts"))
        .and(wiremock::matchers::query_param("status", "running"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;

    Command::cargo_bin("nixfleet")
        .unwrap()
        .args([
            "--control-plane-url",
            &server.uri(),
            "--api-key",
            "test-key",
            "rollout",
            "list",
            "--status",
            "running",
        ])
        .assert()
        .success();
}

// =====================================================================
// rollout status <id> — GET /api/v1/rollouts/{id}
// =====================================================================

#[tokio::test]
async fn rollout_status_renders_detail() {
    let server = cp_mock().await;

    // Shape matches shared/src/rollout.rs::RolloutDetail.
    Mock::given(method("GET"))
        .and(path("/api/v1/rollouts/r-abc123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "r-abc123",
            "status": "completed",
            "strategy": "all_at_once",
            "release_id": "rel-zzz",
            "on_failure": "pause",
            "failure_threshold": "0",
            "health_timeout": 60,
            "batches": [],
            "created_at": "2026-04-11T00:00:00Z",
            "updated_at": "2026-04-11T00:00:00Z",
            "created_by": "test",
            "events": []
        })))
        .mount(&server)
        .await;

    Command::cargo_bin("nixfleet")
        .unwrap()
        .args([
            "--control-plane-url",
            &server.uri(),
            "--api-key",
            "test-key",
            "rollout",
            "status",
            "r-abc123",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("r-abc123"));
}

// =====================================================================
// rollout cancel <id> — POST /api/v1/rollouts/{id}/cancel
// =====================================================================

#[tokio::test]
async fn rollout_cancel_calls_post() {
    let server = cp_mock().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/rollouts/r-abc123/cancel"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    Command::cargo_bin("nixfleet")
        .unwrap()
        .args([
            "--control-plane-url",
            &server.uri(),
            "--api-key",
            "test-key",
            "rollout",
            "cancel",
            "r-abc123",
        ])
        .assert()
        .success();

    let received = server.received_requests().await.unwrap();
    assert!(
        received
            .iter()
            .any(|r| r.method.as_str() == "POST"
                && r.url.path() == "/api/v1/rollouts/r-abc123/cancel"),
        "expected POST cancel request"
    );
}

// =====================================================================
// release show <id> — GET /api/v1/releases/{id}
// (plain `release list` clap routing is dispatched transitively by
// every other release test that goes through the same enum)
// =====================================================================

#[tokio::test]
async fn release_show_renders_detail() {
    let server = cp_mock().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/releases/rel-show-42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "rel-show-42",
            "flake_ref": "github:my-org/fleet",
            "flake_rev": "deadbeef",
            "cache_url": null,
            "host_count": 1,
            "created_at": "2026-04-11T00:00:00Z",
            "created_by": "operator",
            "entries": [
                {
                    "hostname": "web-01",
                    "store_path": "/nix/store/aaa-web-01",
                    "platform": "x86_64-linux",
                    "tags": ["web"],
                }
            ]
        })))
        .mount(&server)
        .await;

    Command::cargo_bin("nixfleet")
        .unwrap()
        .args([
            "--control-plane-url",
            &server.uri(),
            "--api-key",
            "test-key",
            "release",
            "show",
            "rel-show-42",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("rel-show-42"))
        .stdout(predicate::str::contains("web-01"));
}

// =====================================================================
// release diff <a> <b> — GET /api/v1/releases/{a}/diff/{b}
// =====================================================================

#[tokio::test]
async fn release_diff_renders_changes() {
    let server = cp_mock().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/releases/rel-a/diff/rel-b"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "added": ["new-host"],
            "removed": ["old-host"],
            "changed": [],
            "unchanged": ["stable-host"]
        })))
        .mount(&server)
        .await;

    Command::cargo_bin("nixfleet")
        .unwrap()
        .args([
            "--control-plane-url",
            &server.uri(),
            "--api-key",
            "test-key",
            "release",
            "diff",
            "rel-a",
            "rel-b",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("new-host"))
        .stdout(predicate::str::contains("old-host"));
}

// =====================================================================
// --config flag — loads .nixfleet.toml from explicit path
// =====================================================================

#[tokio::test]
async fn config_flag_loads_from_explicit_path() {
    let server = cp_mock().await;

    // Write a config file pointing at the mock server
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join(".nixfleet.toml");
    std::fs::write(
        &config_path,
        format!(
            "[control-plane]\nurl = \"{}\"\n",
            server.uri()
        ),
    )
    .unwrap();

    Mock::given(method("GET"))
        .and(path("/api/v1/machines"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            fake_machine_status_json("cfg-host", &["test"]),
        ])))
        .mount(&server)
        .await;

    // Run from a different directory with --config pointing at the file
    let other_dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("nixfleet")
        .unwrap()
        .current_dir(other_dir.path())
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--api-key",
            "test-key",
            "status",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("cfg-host"));
}

// =====================================================================
// --tags comma-separated — machines list with "web,db"
// =====================================================================

#[tokio::test]
async fn machines_list_comma_separated_tags() {
    let server = cp_mock().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/machines"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            fake_machine_status_json("web-01", &["web"]),
            fake_machine_status_json("db-01", &["db"]),
            fake_machine_status_json("cache-01", &["cache"]),
        ])))
        .mount(&server)
        .await;

    let assert = Command::cargo_bin("nixfleet")
        .unwrap()
        .args([
            "--control-plane-url",
            &server.uri(),
            "--api-key",
            "test-key",
            "machines",
            "list",
            "--tags",
            "web,db",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("web-01"), "expected web-01: {stdout}");
    assert!(stdout.contains("db-01"), "expected db-01: {stdout}");
    assert!(
        !stdout.contains("cache-01"),
        "cache-01 must be excluded: {stdout}"
    );
}

// =====================================================================
// --tags repeatable — machines register with --tags a --tags b
// =====================================================================

#[tokio::test]
async fn machines_register_repeatable_tags() {
    let server = cp_mock().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/machines/node-01/register"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;

    Command::cargo_bin("nixfleet")
        .unwrap()
        .args([
            "--control-plane-url",
            &server.uri(),
            "--api-key",
            "test-key",
            "machines",
            "register",
            "node-01",
            "--tags",
            "web",
            "--tags",
            "prod",
        ])
        .assert()
        .success();

    let received = server.received_requests().await.unwrap();
    let req = received
        .iter()
        .find(|r| r.url.path() == "/api/v1/machines/node-01/register")
        .expect("register request must reach the mock");
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    let tags = body
        .get("tags")
        .and_then(|t| t.as_array())
        .expect("body must have tags array");
    assert!(tags.iter().any(|t| t == "web"), "expected web tag");
    assert!(tags.iter().any(|t| t == "prod"), "expected prod tag");
}
