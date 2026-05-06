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
    /// `rollback-and-halt`. Action-plan record only — the CP-side
    /// checkin pipeline ships the actual `RollbackSignal`.
    RollbackHost {
        rollout: String,
        host: String,
        target_ref: String,
    },
    /// Healthy → Soaked transition once the host has been Healthy for
    /// at least `wave.soak_minutes`.
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
    /// Wave-staging compliance gate held promotion: under `enforce` mode,
    /// at least one host in an earlier wave has outstanding
    /// `ComplianceFailure` / `RuntimeGateError` events under THIS
    /// rollout's id (per-rollout grouping enforces resolution-by-replacement).
    WaveBlocked {
        rollout: String,
        blocked_wave: usize,
        failing_hosts: Vec<String>,
        failing_events_count: usize,
    },
    /// Cross-channel ordering held OpenRollout: a `channelEdges` predecessor
    /// channel has not converged its most-recent rollout. The reconciler
    /// re-checks every tick; the journal-emission is debounced via
    /// `Observed.last_deferrals` so this fires once per (channel, target_ref,
    /// blocked_by) transition rather than every reconcile tick.
    RolloutDeferred {
        channel: String,
        target_ref: String,
        blocked_by: String,
        reason: String,
    },
    /// Declarative key rotation deadline reached: a trust slot's
    /// `retire_at` is past `now` and a `successor` is declared, so
    /// the operator's tooling should rotate `current → previous` and
    /// `successor → current` in the next fleet commit. Emitted from
    /// `check_trust_rotations`, NOT from the main reconcile loop —
    /// the action is informational; trust mutations are out-of-band
    /// (operator-driven git commits, not CP self-mutation). Once
    /// the operator updates fleet.nix and CI signs the new release,
    /// the slot's `successor` field clears and the action stops
    /// firing on subsequent ticks.
    RotateTrustRoot {
        /// Which slot rotated — `"ciReleaseKey"` or `"orgRootKey"`.
        which: String,
        retire_at: chrono::DateTime<chrono::Utc>,
    },
}
