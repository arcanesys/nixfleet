//! NixFleet v0.2 agent library.
//!
//! The v0.2 agent is poll-only. It enrols via bootstrap token, fetches
//! an mTLS client certificate, checks in on cadence, reports
//! `currentGeneration`, and logs the target it *would* activate. It
//! never runs `nixos-rebuild switch`; activation is Phase 4 work gated
//! on Checkpoint 2.
//!
//! See `docs/KICKOFF.md §1 Stream C`, `rfcs/0003-protocol.md §4`, and
//! `docs/trust-root-flow.md §5`.
