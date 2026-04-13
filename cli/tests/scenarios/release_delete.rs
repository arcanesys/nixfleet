//! D3 — `nixfleet release delete` CLI dispatch.
//!
//! The CP-side R5/R6 (delete on referenced → 409, delete on orphan → 204)
//! are already covered by `control-plane/tests/scenarios/release.rs`. This
//! test pins the CLI dispatch shape: exit code + output text per status
//! returned by the CP. We mock the CP with wiremock so the test asserts
//! ONLY the CLI behaviour, not the CP behaviour.

use super::harness::cli_lock;
use assert_cmd::Command;
use predicates::prelude::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn release_delete_orphan_returns_zero_with_confirmation() {
    let _guard = cli_lock().await;
    let server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path("/api/v1/releases/rel-orphan-001"))
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
            "release",
            "delete",
            "rel-orphan-001",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Release rel-orphan-001 deleted"));
}

#[tokio::test]
async fn release_delete_referenced_exits_with_error_message() {
    let _guard = cli_lock().await;
    let server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path("/api/v1/releases/rel-referenced-001"))
        .respond_with(
            ResponseTemplate::new(409).set_body_string("release referenced by rollout rollout-abc"),
        )
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
            "delete",
            "rel-referenced-001",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("still referenced by a rollout"));
}

#[tokio::test]
async fn release_delete_unknown_id_exits_with_not_found_message() {
    let _guard = cli_lock().await;
    let server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path("/api/v1/releases/rel-missing"))
        .respond_with(ResponseTemplate::new(404))
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
            "delete",
            "rel-missing",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[tokio::test]
async fn release_delete_subcommand_is_registered() {
    let _guard = cli_lock().await;
    // Negative: a stale clap definition that omits the Delete variant
    // would surface as "unrecognized subcommand 'delete'". This test
    // confirms the subcommand is dispatchable at the parser level
    // without needing a CP at all.
    Command::cargo_bin("nixfleet")
        .unwrap()
        .args(["release", "delete", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Delete a release"));
}
