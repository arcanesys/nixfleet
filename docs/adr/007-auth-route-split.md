# ADR-007: Auth Route Split — Agent vs Admin

**Status:** Accepted
**Date:** 2026-04-03

## Context

The control plane serves two distinct classes of clients:

- **Agents** — managed hosts that poll for their desired generation, report status, and submit health data. Agents authenticate via mTLS client certificates issued by the fleet CA.
- **Operators** — humans and automation using the CLI or REST API to deploy, query, and manage the fleet. Operators authenticate via API keys.

The original routing wired both agent endpoints and admin endpoints through the same middleware stack, which included API key validation. This broke agent communication: agents present mTLS certificates, not API keys, so they were rejected at the middleware layer before reaching the handler.

## Decision

Split the control plane router into two sub-routers with independent middleware:

- **Agent router** — handles `/api/v1/poll`, `/api/v1/report`, `/api/v1/register`, and `/api/v1/health`. No API key middleware. Authentication is delegated entirely to the TLS layer (mTLS client certificate validation). If mTLS is not configured, these endpoints are unauthenticated at the HTTP layer.
- **Admin router** — handles all other `/api/v1/` routes and the CLI-facing endpoints. API key middleware is required; requests without a valid key receive `401 Unauthorized`.

Shared endpoints (`/health`, `/metrics`) sit outside both routers and require no authentication.

## Consequences

- Agent polling and reporting work correctly when mTLS is enabled — no API key required
- Admin endpoints remain protected; API key enforcement is not weakened
- Adding a new agent-facing endpoint requires explicitly placing it on the agent router
- Deployments without mTLS have unauthenticated agent endpoints — acceptable in trusted networks, documented as a production risk
- The route split is visible in the router wiring, making the auth model explicit rather than implicit
