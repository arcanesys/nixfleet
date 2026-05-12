# nixfleet-agent

**Role.** Host daemon. Runs as `nixfleet-agent.service` (systemd on Linux) / `system.nixfleet.agent` launchd label (Darwin). Polls the control plane over mTLS, fetches dispatched closures from the binary cache, activates them via `nixos-rebuild switch` (or the Darwin equivalent), confirms convergence or reports failure, and self-signs evidence payloads with the host SSH key. Carries the actuator side of the contract: the only thing in the architecture that mutates a host's running system, and the last line of defence (rollback on failed activation, freshness-window enforcement on stale intent).

**Key types and state machines.** `comms::ReqwestReporter` (mTLS-bearing HTTP client to CP), `checkin_state` (in-memory tracker of in-flight checkin / confirm cycles), `evidence_signer` (ed25519 over JCS canonical payload bytes using the host's `/etc/ssh/ssh_host_ed25519_key`), `manifest_cache` (persisted last-good `RolloutManifest` for offline-tolerant verification), `freshness` (clock-skew check against signed-at vs the channel's freshness window), `recovery` (boot-time check-for-rollback path), `compliance` (probe runner that emits signed evidence). The host lifecycle is implicit in the call sequence: Idle -> Fetching -> Activating -> Confirming -> Healthy, with auto-rollback on activation failure and CP-driven rollback signal handling.

**Surface.** Binary `nixfleet-agent` with CLI flags mirrored by environment variables (`NIXFLEET_AGENT_*`): `--control-plane-url`, `--machine-id` (must match the client-cert CN), `--poll-interval` (default 60s), `--trust-file`, `--ca-cert`, `--client-cert`, `--client-key`, `--bootstrap-token-file` (enrol via `/v1/enroll` when client-cert is absent), `--state-dir` (default `/var/lib/nixfleet-agent`), `--compliance-gate-mode`, `--ssh-host-key-file`, `--health-checks-config`. NixOS module `services.nixfleet-agent.{enable, controlPlaneUrl, machineId, trustFile, tls.{caCert, clientCert}, healthChecks, ...}` materialises the systemd unit and renders the health-checks JSON.

**Links.**

- Generated rustdoc: [`api/nixfleet_agent/`](../../api/nixfleet_agent/index.html)
- Relevant RFCs: [RFC-0002](../../rfcs/0002-reconciler.md), [RFC-0003](../../rfcs/0003-protocol.md), [RFC-0006](../../rfcs/0006-freshness-window-policy.md), [RFC-0007](../../rfcs/0007-air-gapped-operation.md)
- Architecture component: [§1.5 Agent](../../design/architecture.md#15-agent-the-actuator), [§3 The main flow](../../design/architecture.md#3-the-main-flow)
