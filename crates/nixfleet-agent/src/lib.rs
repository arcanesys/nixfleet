//! NixFleet v0.2 agent library.
//!
//! Phase 3 PR-3: poll-only agent. Reads cert paths + CP URL from CLI
//! flags (set by the NixOS module), builds an mTLS reqwest client,
//! polls `/v1/agent/checkin` every `pollInterval` seconds with a
//! richer body than RFC-0003 §4.1's minimum (pending generation,
//! last fetch outcome, agent uptime). On fetch/verify failures
//! (none happen yet in PR-3 since there's no fetch path until PR-4
//! introduces the observed projection from check-ins), the agent
//! posts to `/v1/agent/report`.
//!
//! It never runs `nixos-rebuild switch`; activation is Phase 4 work
//! gated on Checkpoint 2.
//!
//! See `docs/KICKOFF.md §1 Stream C`, `rfcs/0003-protocol.md §4`,
//! and `docs/trust-root-flow.md §5`.

pub mod checkin_state;
pub mod comms;
pub mod enrollment;
