# nixfleet-cli

**Role.** Operator umbrella binary (`nixfleet`). Talks to the control plane over mTLS for status / trace, and ships offline helpers for bootstrap and key derivation. Library form so binaries compose against it and unit tests exercise table rendering and status classification without spinning up a real CP. Carries the operator's day-to-day surface; everything an operator types lives behind one of these subcommands.

**Key types and state machines.** `ResolvedClientConfig` (cp_url + CA / client cert / client key paths after the flag > env > file layered loader runs), `FileConfig` / `Overrides` (config-file shape and per-layer overrides), `StatusInputs` (`now`, `hosts`, `channel_freshness`) feeding the deterministic table renderer. Status-label priority is explicit (Failed > Quarantined > PendingReboot > Converged > Stale > InFlight > Queued) with tests locking each transition; pin metadata appends as a `🔒<short>` suffix so health stays the primary signal.

**Surface.** Subcommands of the `nixfleet` binary: `status [--json] [--no-color]` (fleet table: convergence, freshness, outstanding compliance / runtime-gate / health failures, pin markers), `rollout trace <rollout-id> [--json]` (wave-major dispatch history with `<open>` markers for unresolved dispatches), `config init --cp-url --ca-cert --client-cert --client-key [--path] [--force]` (write `~/.config/nixfleet/config.toml`), `derive-pubkey` (base64 ed25519 pubkey from raw private key file), `mint-operator-cert` (mTLS client cert from the offline fleet root CA), `mint-token` (bootstrap token for first-boot enrolment). Network calls hit `/v1/hosts`, `/v1/channels/{name}`, `/v1/rollouts/{id}/trace`.

**Links.**

- Generated rustdoc: [`api/nixfleet_cli/`](../../api/nixfleet_cli/index.html)
- Relevant RFCs: [RFC-0003](../../rfcs/0003-protocol.md), [RFC-0005](../../rfcs/0005-trust-lifecycle.md)
- Architecture component: [§1.4 Control plane](../../design/architecture.md#14-control-plane-the-router), [§3 The main flow](../../design/architecture.md#3-the-main-flow)
