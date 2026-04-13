# Security Policy

## Reporting Vulnerabilities

**Do not open a public issue for security vulnerabilities.**

Instead, use one of these methods:

1. **GitHub Security Advisory** (preferred): Go to the [Security tab](https://github.com/arcanesys/nixfleet/security/advisories/new) and click "Report a vulnerability"
2. **Email:** security@arcanesys.fr

Include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

## Auth Route Split

The control plane separates agent-facing routes from admin routes:

- **Agent routes** (`/api/v1/machines/{id}/desired-generation`, `/api/v1/machines/{id}/report`): authenticated via mTLS client certificate. No API key required.
- **Admin routes** (all other `/api/v1/...` endpoints): authenticated via API key (Bearer token). When `--client-ca` is set, admin clients also require a valid client certificate.

This split ensures API key rotation does not affect deployed agents, and machine credentials cannot reach admin endpoints.

## Scope

The following are in scope for security reports:

- Control plane authentication and authorization (API keys, mTLS)
- Agent-to-control-plane communication security (including route separation bypass)
- Rollout orchestration logic (e.g., bypassing rollout protections)
- Secret handling in Nix modules
- SQL injection or data exposure in SQLite queries
- Privilege escalation in agent or control plane systemd services

## Response Timeline

- **Acknowledge:** within 48 hours
- **Assess severity:** within 1 week
- **Fix critical issues:** within 2 weeks
- **Coordinate disclosure:** timeline agreed with reporter

## Supported Versions

Security fixes are applied to the latest release only.
