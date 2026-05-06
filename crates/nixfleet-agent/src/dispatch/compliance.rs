//! DispatchCtx-shaped wrappers over the lib gate orchestration in
//! `nixfleet_agent::compliance`. The lib variant takes primitive inputs so
//! both dispatch and boot-recovery can share the same posting + rollback path.

use nixfleet_proto::agent_wire::EvaluatedTarget;

use nixfleet_agent::comms::Reporter;

use crate::Args;

use super::DispatchCtx;

pub(super) async fn run_runtime_gate(
    target: &EvaluatedTarget,
    args: &Args,
    activation_completed_at: chrono::DateTime<chrono::Utc>,
) -> (
    nixfleet_agent::compliance::GateMode,
    nixfleet_agent::compliance::GateOutcome,
) {
    let resolved_mode = nixfleet_agent::compliance::resolve_runtime_gate_mode(
        target.compliance_mode.as_deref(),
        args.compliance_gate_mode.as_deref(),
    )
    .await;
    let gate_outcome = nixfleet_agent::compliance::run_runtime_gate(
        activation_completed_at,
        &nixfleet_agent::compliance::default_evidence_path(),
        resolved_mode,
    )
    .await;
    (resolved_mode, gate_outcome)
}

/// Returns `true` iff the agent should skip confirm and stay rolled back.
pub(super) async fn process_gate_outcome<R: Reporter>(
    gate_outcome: &nixfleet_agent::compliance::GateOutcome,
    resolved_mode: nixfleet_agent::compliance::GateMode,
    ctx: &DispatchCtx<'_, R>,
    activation_completed_at: chrono::DateTime<chrono::Utc>,
) -> bool {
    nixfleet_agent::compliance::apply_gate_outcome(
        gate_outcome,
        resolved_mode,
        &ctx.args.machine_id,
        &ctx.target.channel_ref,
        ctx.reporter,
        ctx.evidence_signer,
        activation_completed_at,
    )
    .await
}
