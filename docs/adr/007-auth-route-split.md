# ADR-007: Auth Route Split -- Agent vs Admin

**Status:** Accepted
**Date:** 2026-04-03

## Context

The control plane serves two distinct classes of clients:

- **Agents** -- managed hosts that poll for their desired generation, report status, and submit health data. Agents authenticate via mTLS client certificates issued by the fleet CA.
- **Operators** -- humans and automation using the CLI or REST API to deploy, query, and manage the fleet. Operators authenticate via API keys.

A single middleware stack for both client types doesn't work: agents present mTLS certificates, not API keys, so they get rejected before reaching the handler.

## Decision

Split the control plane router into two sub-routers with independent middleware:

- **Agent router** -- handles `/api/v1/machines/{id}/desired-generation` and `/api/v1/machines/{id}/report`. No API key middleware. Authentication is delegated entirely to the TLS layer (mTLS client certificate validation). If mTLS is not configured, these endpoints are unauthenticated at the HTTP layer.
- **Admin router** -- handles all other `/api/v1/` routes (machine management, rollouts, audit). API key middleware is required; requests without a valid key receive `401 Unauthorized`.

Shared endpoints (`/health`, `/metrics`) sit outside both routers and require no authentication.

### TLS model

When `--client-ca` is set, the control plane requires client certificates from **all** TLS connections (required mTLS). This means admin clients must present a valid client certificate **in addition to** an API key. This is defense-in-depth: the TLS layer authenticates the connection, the API key authorizes the operation.

Optional mTLS (allowing admin clients without a client cert) is not viable because `axum-server` does not expose peer certificate information to the application layer. Required mTLS for all connections is the simpler, more secure approach.

**Future consideration:** When the `nixfleet` CLI needs to connect without a client cert (e.g., from a developer laptop), the CP can be fronted by a reverse proxy that handles mTLS termination, or the TLS layer can be migrated to a manual accept loop with `tokio-rustls` that supports optional mTLS with per-route enforcement.

## Consequences

- Agent polling and reporting work correctly when mTLS is enabled -- no API key required
- Admin endpoints are double-authenticated: mTLS at the transport layer + API key at the application layer
- Admin tools (CLI, curl) must have access to a client certificate signed by the fleet CA
- Adding a new agent-facing endpoint requires explicitly placing it on the agent router
- Deployments without mTLS have unauthenticated agent endpoints -- acceptable in trusted networks, documented as a production risk
- The route split is visible in the router wiring, making the auth model explicit rather than implicit
