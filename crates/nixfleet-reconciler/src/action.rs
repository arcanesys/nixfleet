use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    OpenRollout {
        channel: String,
        target_ref: String,
    },
    DispatchHost {
        rollout: String,
        host: String,
        target_ref: String,
    },
    PromoteWave {
        rollout: String,
        new_wave: usize,
    },
    ConvergeRollout {
        rollout: String,
    },
    HaltRollout {
        rollout: String,
        reason: String,
    },
    /// Emitted alongside `HaltRollout` for Failed hosts under
    /// `rollback-and-halt`. Action-plan record only - the CP-side checkin
    /// pipeline ships the actual `RollbackSignal`.
    RollbackHost {
        rollout: String,
        host: String,
        target_ref: String,
    },
    /// Healthy -> Soaked transition (host has been Healthy for `soak_minutes`).
    SoakHost {
        rollout: String,
        host: String,
    },
    /// Observability-only: rollout references a channel no longer in
    /// `fleet.resolved.channels`. Reconciler silently continues.
    ChannelUnknown {
        channel: String,
    },
    Skip {
        host: String,
        reason: String,
    },
    /// Compliance gate held wave promotion under `enforce`. Per-rollout
    /// grouping enforces resolution-by-replacement.
    WaveBlocked {
        rollout: String,
        blocked_wave: usize,
        failing_hosts: Vec<String>,
        failing_events_count: usize,
    },
    /// Cross-channel ordering held OpenRollout: a `channelEdges` predecessor
    /// channel hasn't converged. Debounced via `Observed.last_deferrals` so
    /// this fires once per (channel, target_ref, blocked_by) transition
    /// rather than every tick.
    RolloutDeferred {
        channel: String,
        target_ref: String,
        blocked_by: String,
        reason: String,
    },
    /// Trust slot's `retire_at` reached with a `successor` declared.
    /// Informational only - trust mutations are out-of-band (operator
    /// commits, not CP self-mutation). Stops firing once the operator
    /// rotates fleet.nix and the new release lands.
    RotateTrustRoot {
        which: String,
        retire_at: chrono::DateTime<chrono::Utc>,
    },
}
