//! Agent half of the polling contract.
//!
//! The CP-side is pinned in `control-plane/tests/polling_scenarios.rs`:
//! the CP's `desired-generation` response carries `poll_hint = Some(5)` iff
//! the machine is part of an active rollout. These tests pin the agent
//! half: when the agent sees `poll_hint`, does it actually shorten its
//! next poll? When the hint clears, does it revert to the configured
//! `poll_interval`?
//!
//! Cadence is asserted in real time with sub-second intervals.
//! `tokio::test(start_paused)` + `tokio::time::advance` deadlock against
//! reqwest's I/O timers and wiremock's listener under paused time, so
//! the test must use real time. The contract is the same: configured
//! `poll_interval` is set MUCH longer than the hint, and the test
//! asserts the observed cadence matches the hint.

use nixfleet_agent::Config;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn base_config(server_url: String, db_path: String) -> Config {
    Config {
        control_plane_url: server_url,
        machine_id: "web-01".to_string(),
        // Long steady-state poll interval so the test can prove the
        // hint actually shortened it (otherwise we'd see polls anyway
        // even at the steady-state rate).
        poll_interval: Duration::from_secs(60),
        retry_interval: Duration::from_secs(30),
        cache_url: None,
        db_path,
        // dry_run skips the apply path so the test does not need a
        // working `switch-to-configuration` binary on the host.
        dry_run: true,
        // Wiremock listens on http:// — allow it.
        allow_insecure: true,
        ca_cert: None,
        client_cert: None,
        client_key: None,
        // Path that does not exist; HealthRunner falls back to the
        // platform default health check.
        health_config_path: "/dev/null".to_string(),
        // Make the health tick fire effectively never so it does not
        // pollute the poll-cadence count.
        health_interval: Duration::from_secs(3600),
        tags: vec![],
        metrics_port: None,
    }
}

/// Count the GET /desired-generation requests recorded by the mock.
async fn poll_count(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path().contains("/desired-generation"))
        .count()
}

/// Read whatever `nix::current_generation()` returns in the current
/// environment. The mock uses this as the desired-generation hash so
/// every poll immediately hits the "Already at desired generation"
/// early-return branch in `run_deploy_cycle`, regardless of whether
/// the test runs on a real dev host (with `/run/current-system`) or
/// inside a nix build sandbox (where that symlink doesn't exist and
/// `current_generation` returns `""`).
///
/// Why this matters: without this alignment, the loop proceeds past
/// the early-return and calls `nix::fetch_closure()`, which in turn
/// runs `nix copy` / `nix-store --realise` against the mock's store
/// path. That call fails deterministically in sandbox builds (no
/// network, no real store path) and the loop returns
/// `PollOutcome::Failed`, which schedules the next tick at
/// `retry_interval = 30s` instead of `poll_hint = 1s`. Only one poll
/// fires in the 2.5s observation window and the test fails with
/// "got 1 polls, expected ≥3".
///
/// With the mock hash tied to `current_generation()`, every poll
/// takes the early-return path and the `poll_hint` cadence is the
/// only thing under test — which is exactly what we want to pin.
async fn current_generation_for_mock() -> String {
    nixfleet_agent::nix::current_generation()
        .await
        .unwrap_or_default()
}

/// P-agent-1 — when the CP returns `poll_hint = Some(1)` (1 second),
/// the agent's next poll fires at 1 second, NOT at the configured
/// 60-second `poll_interval`. We observe over a 2.5-second window and
/// expect ≥ 3 polls (initial + ≥ 2 hint-driven). Without the hint we
/// would see at most 1 poll (the initial one) in that window.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn poll_hint_shortens_next_interval() {
    let server = MockServer::start().await;
    let current = current_generation_for_mock().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/machines/web-01/desired-generation"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "hash": current,
            "cache_url": null,
            "poll_hint": 1
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/machines/web-01/report"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("agent.db").to_string_lossy().into_owned();
    let cfg = base_config(server.uri(), db_path);

    let handle = tokio::spawn(async move {
        let _ = nixfleet_agent::run_loop(cfg).await;
    });

    // Observe over 2.5 seconds. With poll_hint=1s, expect at least
    // 3 polls: initial (t=0) + t≈1s + t≈2s. With the configured 60s
    // interval and no hint, we'd only see the initial poll.
    tokio::time::sleep(Duration::from_millis(2500)).await;

    let count = poll_count(&server).await;
    handle.abort();
    let _ = handle.await;

    assert!(
        count >= 3,
        "expected ≥3 polls within 2.5s under poll_hint=1; got {count}. \
         Without the hint the configured 60s poll_interval would only \
         allow the initial poll in this window."
    );
}

/// P-agent-2 — when the CP stops sending `poll_hint`, the agent reverts
/// to its configured `poll_interval`. The mock returns `poll_hint=1`
/// for the first 2 polls then `null` afterwards. After the hint clears
/// the agent must NOT poll again within the configured interval.
///
/// Configured `poll_interval = 5s` for this test. Observation:
///   - Phase A (0..2s): expect ≥ 2 polls under hint=1.
///   - Phase B (2..3.5s): hint cleared on the 2nd hint poll's response;
///     after that response is processed the next interval is rebuilt
///     to 5s, so within ~1.5s we should see no new polls.
///   - Phase C (after ≥ 5s elapsed since the hint-cleared poll):
///     a new poll should fire.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn poll_hint_clearing_reverts_to_configured_interval() {
    let server = MockServer::start().await;
    let current = current_generation_for_mock().await;

    // First two polls return poll_hint=1 (1 second). Mock matches in
    // registration order; up_to_n_times(2) makes this mock match
    // twice then yield to the next mock.
    //
    // `hash` matches the current environment's current_generation
    // output (see current_generation_for_mock above for why).
    Mock::given(method("GET"))
        .and(path("/api/v1/machines/web-01/desired-generation"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "hash": current,
            "cache_url": null,
            "poll_hint": 1
        })))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    // Subsequent polls: poll_hint absent.
    Mock::given(method("GET"))
        .and(path("/api/v1/machines/web-01/desired-generation"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "hash": current,
            "cache_url": null,
            "poll_hint": null
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/machines/web-01/report"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("agent.db").to_string_lossy().into_owned();
    let mut cfg = base_config(server.uri(), db_path);
    // Short steady-state interval — long enough to be distinct from
    // the 1s hint, short enough to observe within a tractable test.
    cfg.poll_interval = Duration::from_secs(5);

    let handle = tokio::spawn(async move {
        let _ = nixfleet_agent::run_loop(cfg).await;
    });

    // Phase A: 2.5 seconds — expect ≥ 3 polls under the hint
    // (initial + 2 hint-driven). The 3rd poll's response has hint=null.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let phase_a = poll_count(&server).await;
    assert!(
        phase_a >= 3,
        "phase A: expected ≥3 polls within 2.5s; got {phase_a}"
    );

    // Phase B: 1.5 seconds of quiet. After the hint cleared, the next
    // poll is scheduled at +5s from the third (hint=null) poll. So
    // within 1.5s we should see no NEW polls.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let phase_b = poll_count(&server).await;
    assert_eq!(
        phase_b, phase_a,
        "phase B: agent must NOT poll within 1.5s after hint clears \
         (configured poll_interval=5s); phase_a={phase_a}, phase_b={phase_b}"
    );

    // Phase C: another 4 seconds. By now ≥ 5s has elapsed since the
    // last poll, so the configured interval should have fired at
    // least once.
    tokio::time::sleep(Duration::from_millis(4000)).await;
    let phase_c = poll_count(&server).await;
    handle.abort();
    let _ = handle.await;

    assert!(
        phase_c > phase_b,
        "phase C: agent must poll once after the configured 5s interval \
         elapses; phase_b={phase_b}, phase_c={phase_c}"
    );
}
