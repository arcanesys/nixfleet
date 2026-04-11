//! Machine registration, lifecycle, and report-cycle scenarios.
//!
//! | Name | Covers |
//! |---|---|
//! | M1 | `active → decommissioned` via HTTP + rollout target filter |
//! | M2 | Auto-registration on first report + tag propagation |
//! | M3 | Direct desired-generation ↔ report cycle (non-rollout path) |
//! | M4 | `success=false` report maps to `system_state=error` |
//! | M5 | Three-machine desired-generation isolation |
//! | M6 | `Pending → Active` auto-transition on first report |
//! | M7 | `Active ↔ Maintenance` round-trip via PATCH lifecycle |
//!
//! Every scenario spins up a fresh in-process CP via `harness::spawn_cp`
//! and drives it over real HTTP.

#[path = "harness.rs"]
mod harness;

use nixfleet_types::{DesiredGeneration, MachineStatus, Report};

#[tokio::test]
async fn m1_lifecycle_register_active_decommissioned() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "web-01", &["web"]).await;

    // Sanity: freshly registered machine is in the rollout target set for "web".
    let initial = cp.db.get_machines_by_tags(&["web".to_string()]).unwrap();
    assert!(
        initial.contains(&"web-01".to_string()),
        "active web-01 must appear in get_machines_by_tags('web') before decommission"
    );

    // Move to decommissioned via PATCH.
    let resp = cp
        .admin
        .patch(format!("{}/api/v1/machines/web-01/lifecycle", cp.base))
        .json(&serde_json::json!({"lifecycle": "decommissioned"}))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "PATCH lifecycle failed: {} {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );

    let machines = cp.db.list_machines().unwrap();
    let web = machines
        .iter()
        .find(|m| m.machine_id == "web-01")
        .expect("web-01 must still exist after decommission");
    assert_eq!(web.lifecycle, "decommissioned");

    // Negative: `get_machines_by_tags` must NOT return a decommissioned machine
    // when used as a rollout target. If the DB query returns it, any rollout
    // built for `tag=web` would accidentally target a decommissioned host.
    let targets = cp.db.get_machines_by_tags(&["web".to_string()]).unwrap();
    assert!(
        !targets.contains(&"web-01".to_string()),
        "decommissioned machine must NOT appear in get_machines_by_tags('web'); got {targets:?}"
    );
}

#[tokio::test]
async fn m2_tag_sync_from_report_enables_tag_filter() {
    let cp = harness::spawn_cp().await;

    // Machine not pre-registered — post_report should auto-register it and
    // persist its tags.
    harness::fake_agent_report(
        &cp,
        "auto-01",
        "/nix/store/m2-auto-01",
        true,
        "ok",
        &["auto", "canary"],
    )
    .await;

    let tags = cp.db.get_machine_tags("auto-01").unwrap();
    assert!(
        tags.contains(&"auto".to_string()),
        "reported tag 'auto' must be persisted; got {tags:?}"
    );
    assert!(
        tags.contains(&"canary".to_string()),
        "reported tag 'canary' must be persisted; got {tags:?}"
    );

    // `get_machines_by_tags(["auto"])` must surface this machine.
    let found = cp.db.get_machines_by_tags(&["auto".to_string()]).unwrap();
    assert!(
        found.contains(&"auto-01".to_string()),
        "auto-01 must be discoverable via tag 'auto'; got {found:?}"
    );

    // Negative: filtering by a tag the agent did NOT report must return empty.
    let empty = cp
        .db
        .get_machines_by_tags(&["nonexistent".to_string()])
        .unwrap();
    assert!(
        empty.is_empty(),
        "unknown tag must return empty set; got {empty:?}"
    );
}

/// Helper: set a machine's desired_generation directly (DB + in-memory
/// fleet state) without going through the rollout executor. This is the
/// path the CP exposes for "one-off push a specific closure to one
/// machine" — it bypasses batches, health gates, and rollouts entirely.
async fn set_desired_gen(cp: &harness::Cp, machine_id: &str, hash: &str) {
    cp.db.set_desired_generation(machine_id, hash).unwrap();
    let mut fleet = cp.fleet.write().await;
    let machine = fleet.get_or_create(machine_id);
    machine.desired_generation = Some(DesiredGeneration {
        hash: hash.to_string(),
        cache_url: None,
        poll_hint: None,
    });
}

/// M3 — end-to-end agent ↔ CP cycle via the NON-rollout path.
///
/// The `/desired-generation` + `/report` contract has to work even
/// when the operator pushes a generation directly (no rollout, no
/// batches, no health gate). That path is exercised by every agent
/// on startup and is distinct from the rollout-driven deploy path
/// covered by `deploy_scenarios.rs`.
#[tokio::test]
async fn m3_direct_desired_gen_report_cycle() {
    let cp = harness::spawn_cp().await;
    let gen_hash = "/nix/store/m3-nixos-system";

    // 1. Unknown machine → 404.
    let resp = cp
        .admin
        .get(format!(
            "{}/api/v1/machines/m3-host/desired-generation",
            cp.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404, "unknown machine must 404");

    // 2. Push a desired generation directly.
    set_desired_gen(&cp, "m3-host", gen_hash).await;

    // 3. Agent polls and receives the generation.
    let desired: DesiredGeneration = cp
        .admin
        .get(format!(
            "{}/api/v1/machines/m3-host/desired-generation",
            cp.base
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(desired.hash, gen_hash);
    assert!(desired.cache_url.is_none());

    // 4. Agent reports success.
    harness::fake_agent_report(&cp, "m3-host", gen_hash, true, "applied", &[]).await;

    // 5. Inventory reflects both desired and current.
    let machines: Vec<MachineStatus> = cp
        .admin
        .get(format!("{}/api/v1/machines", cp.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let m = machines
        .iter()
        .find(|m| m.machine_id == "m3-host")
        .expect("m3-host in inventory");
    assert_eq!(m.desired_generation.as_deref(), Some(gen_hash));
    assert_eq!(m.current_generation, gen_hash);
    assert_eq!(m.system_state, "ok", "success=true maps to system_state=ok");
}

/// M4 — `success=false` report maps to `system_state="error"`.
///
/// Covers the reporting side of RB1: when an agent reports a failed
/// apply, the CP's inventory must reflect the error state and must
/// NOT advance `desired_generation` — the operator has to explicitly
/// pick a new target.
#[tokio::test]
async fn m4_failed_report_transitions_state_to_error() {
    let cp = harness::spawn_cp().await;
    let bad_hash = "/nix/store/m4-bad";
    let rollback_target = "/nix/store/m4-good";

    set_desired_gen(&cp, "m4-host", bad_hash).await;

    // Agent tries bad_hash, health check fails, agent rolls back to
    // the previous good generation and reports success=false.
    let report = Report {
        machine_id: "m4-host".to_string(),
        current_generation: rollback_target.to_string(),
        success: false,
        message: "rolled back after health check failure".to_string(),
        timestamp: chrono::Utc::now(),
        tags: vec![],
        health: None,
    };
    cp.admin
        .post(format!("{}/api/v1/machines/m4-host/report", cp.base))
        .json(&report)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let machines: Vec<MachineStatus> = cp
        .admin
        .get(format!("{}/api/v1/machines", cp.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let m = machines.iter().find(|m| m.machine_id == "m4-host").unwrap();
    assert_eq!(
        m.system_state, "error",
        "failed report must map to system_state=error"
    );
    assert_eq!(
        m.current_generation, rollback_target,
        "current_generation reflects the rollback target"
    );
    assert_eq!(
        m.desired_generation.as_deref(),
        Some(bad_hash),
        "desired_generation unchanged — operator picks the next target explicitly"
    );
}

/// M5 — three machines must not leak desired generations into each other.
///
/// Regression guard against the in-memory FleetState or DB query
/// accidentally returning the wrong row for a given machine id.
#[tokio::test]
async fn m5_multi_machine_desired_gen_isolation() {
    let cp = harness::spawn_cp().await;
    let pairs = [
        ("iso-web-01", "/nix/store/iso-aaa"),
        ("iso-dev-01", "/nix/store/iso-bbb"),
        ("iso-mac-01", "/nix/store/iso-ccc"),
    ];
    for (id, hash) in &pairs {
        set_desired_gen(&cp, id, hash).await;
    }

    for (id, hash) in &pairs {
        let desired: DesiredGeneration = cp
            .admin
            .get(format!(
                "{}/api/v1/machines/{id}/desired-generation",
                cp.base
            ))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(&desired.hash, hash, "machine {id} must see only its own gen");
    }

    let list: Vec<MachineStatus> = cp
        .admin
        .get(format!("{}/api/v1/machines", cp.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.len(), 3, "all three machines visible");
}

/// M6 — Pending → Active auto-transition on the first successful report.
///
/// A freshly registered `Pending` machine must auto-promote to `Active`
/// as soon as it posts a successful report. Covers the HTTP layer of
/// the lifecycle state machine; the pure enum matrix is tested at the
/// unit level in `shared/src/lib.rs`.
#[tokio::test]
async fn m6_pending_auto_activates_on_first_report() {
    let cp = harness::spawn_cp().await;

    // Register explicitly as Pending.
    cp.admin
        .post(format!("{}/api/v1/machines/m6-host/register", cp.base))
        .json(&serde_json::json!({"lifecycle": "pending"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let before: Vec<MachineStatus> = cp
        .admin
        .get(format!("{}/api/v1/machines", cp.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let m_before = before.iter().find(|m| m.machine_id == "m6-host").unwrap();
    assert_eq!(
        m_before.lifecycle,
        nixfleet_types::MachineLifecycle::Pending,
        "registration with lifecycle=pending must persist as Pending"
    );

    // First successful report.
    harness::fake_agent_report(
        &cp,
        "m6-host",
        "/nix/store/m6-first",
        true,
        "initial boot",
        &[],
    )
    .await;

    let after: Vec<MachineStatus> = cp
        .admin
        .get(format!("{}/api/v1/machines", cp.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let m_after = after.iter().find(|m| m.machine_id == "m6-host").unwrap();
    assert_eq!(
        m_after.lifecycle,
        nixfleet_types::MachineLifecycle::Active,
        "first successful report must auto-promote Pending → Active"
    );
}

/// M7 — Active ↔ Maintenance round trip via PATCH lifecycle.
///
/// Maintenance is the one reversible lifecycle transition — Active can
/// enter it and return to Active without decommissioning. Pinning both
/// legs keeps the HTTP lifecycle endpoint honest.
#[tokio::test]
async fn m7_active_maintenance_round_trip() {
    let cp = harness::spawn_cp().await;
    harness::register_machine(&cp, "m7-host", &[]).await;

    // Active → Maintenance.
    let to_maint = cp
        .admin
        .patch(format!("{}/api/v1/machines/m7-host/lifecycle", cp.base))
        .json(&serde_json::json!({"lifecycle": "maintenance"}))
        .send()
        .await
        .unwrap();
    assert_eq!(to_maint.status(), 200, "active → maintenance must 200");

    // Maintenance → Active.
    let to_active = cp
        .admin
        .patch(format!("{}/api/v1/machines/m7-host/lifecycle", cp.base))
        .json(&serde_json::json!({"lifecycle": "active"}))
        .send()
        .await
        .unwrap();
    assert_eq!(to_active.status(), 200, "maintenance → active must 200");
}
