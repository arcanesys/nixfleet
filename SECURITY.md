# Security Policy

## Reporting Vulnerabilities

**Do not open a public issue for security vulnerabilities.**

Instead, use one of these methods:

1. **GitHub Security Advisory** (preferred): Go to the [Security tab](https://github.com/your-org/nixfleet/security/advisories/new) and click "Report a vulnerability"
2. **Email:** [TBD — add security contact email before publishing]

Include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

## Scope

The following are in scope for security reports:

- Control plane authentication and authorization (API keys, mTLS)
- Agent-to-control-plane communication security
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
