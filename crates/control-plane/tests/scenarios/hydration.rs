//! CP restart mid-rollout resumes from DB state (see ADR 010).
//!
//! The rollout executor re-queries the `rollouts` table on every tick via
//! `list_rollouts_by_status("running")`; it does NOT cache rollouts in
//! `FleetState`. Machines are hydrated from the database on startup via
//! `state::hydrate_from_db`. Together these two properties let a second
//! CP process pick up a rollout started by a first CP process and drive
//! it to completion.
//!
//! This scenario validates that contract:
//!   1. cp1 creates a rollout, drives it to `deploying`, and records a
//!      successful report for the first of two target machines.
//!   2. cp2 is spawned against the SAME on-disk SQLite file. Its fleet
//!      state is hydrated from the DB. It records the second agent's
//!      successful report and ticks the rollout to `completed`.
//!   3. The rollout id observed on cp2 matches the id created on cp1,
//!      proving cp2 did not start from a fresh in-memory cache.

use super::harness;

use nixfleet_control_plane::state;
use nixfleet_types::rollout::{OnFailure, RolloutStatus, RolloutStrategy};

#[tokio::test]
async fn cp_restart_mid_rollout_resumes_from_db() {
    // ---- Stage 1: start CP #1, create a rollout, drive it to deploying ----
    let cp1 = harness::spawn_cp().await;

    harness::register_machine(&cp1, "web-01", &["web"]).await;
    harness::register_machine(&cp1, "web-02", &["web"]).await;

    let release_id = harness::create_release(
        &cp1,
        &[
            ("web-01", "/nix/store/f6-web-01"),
            ("web-02", "/nix/store/f6-web-02"),
        ],
    )
    .await;
    let rollout_id = harness::create_rollout_for_tag(
        &cp1,
        &release_id,
        "web",
        RolloutStrategy::AllAtOnce,
        None,
        "0",
        OnFailure::Pause,
        60,
    )
    .await;

    // First tick: pending → deploying.
    harness::tick_once(&cp1).await;

    // Stage the first half of the reports on cp1.
    harness::agent_reports_health(&cp1, "web-01", "/nix/store/f6-web-01", true).await;

    // ---- Stage 2: simulate restart — spawn cp2 against the same DB ----
    //
    // Keep cp1 alive for the rest of the test. Its `TempDir` owns the
    // SQLite file on disk; dropping cp1 while cp2 is still running would
    // delete the file out from under cp2. SQLite supports multiple
    // connections to the same file (WAL mode), so cp1 and cp2 can
    // coexist. Everything after this line interacts exclusively with cp2.
    let db_path = cp1.db_path.clone();
    let _keep_cp1_alive = cp1;

    let cp2 = harness::spawn_cp_at(Some(&db_path)).await;

    // The harness does not invoke `hydrate_from_db` on spawn (production
    // main.rs does — see control-plane/src/main.rs). Replicate what a
    // real CP restart does by calling it explicitly here.
    state::hydrate_from_db(&cp2.fleet, &cp2.db)
        .await
        .expect("hydrate_from_db");

    // Hydration must have loaded web-01 and web-02 from the DB into
    // cp2's in-memory FleetState.
    let db_machines = cp2.db.list_machines().expect("list_machines").len();
    assert_eq!(db_machines, 2, "cp2 must see the 2 machines in SQLite");
    let fleet_machines = cp2.fleet.read().await.machines.len();
    assert!(
        fleet_machines >= 2,
        "cp2.fleet.machines must be populated by hydrate_from_db (got {fleet_machines})"
    );

    // Complete the rollout on cp2.
    harness::agent_reports_health(&cp2, "web-02", "/nix/store/f6-web-02", true).await;

    harness::tick_once(&cp2).await; // → succeeded
    harness::tick_once(&cp2).await; // → completed

    let detail = harness::wait_rollout_status(
        &cp2,
        &rollout_id,
        RolloutStatus::Completed,
        std::time::Duration::from_secs(2),
    )
    .await;
    assert!(matches!(detail.status, RolloutStatus::Completed));

    // Load-bearing assertion: the rollout row cp2 advanced is the SAME
    // id cp1 created. If cp2 had started from a fresh in-memory cache
    // instead of re-querying the DB per tick, this would not match.
    assert_eq!(
        detail.id, rollout_id,
        "cp2 must resume the same rollout id (re-queried from DB, not cached in FleetState)"
    );
}
