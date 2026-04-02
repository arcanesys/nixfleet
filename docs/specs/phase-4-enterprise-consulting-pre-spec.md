# Phase 4: Consulting & Enterprise — Pre-Spec

**Status:** Vision document — not a design spec. This captures the long-term direction. Details will be refined when Phase 3 is substantially complete.
**Date:** 2026-04-02
**Depends on:** Phase 3 (framework infrastructure) in progress or complete

## Goal

Position nixfleet as the NIS2-compliant infrastructure management platform for European organizations. Revenue through consulting engagements (5-10 machine fleets) and enterprise features (licensed, beyond open source).

## Two Revenue Streams

### Stream 1: Consulting

**Target:** European SMEs needing NIS2 compliance for their IT infrastructure. 5-10 machines, currently managed ad-hoc (Ansible, manual SSH, or nothing).

**Offering:**
- Infrastructure audit (current state assessment)
- NixFleet deployment (declarative, reproducible, auditable)
- Compliance reporting (machine inventory, update cadence, drift detection)
- Ongoing maintenance contract

**What nixfleet must provide:**
- Working open-source platform (Phase 2-3)
- Compliance reporting (see below)
- Getting-started experience under 15 minutes
- Documentation good enough that the client's team can operate independently

**What doesn't need to be built:**
- Multi-tenant — consulting clients each get their own CP instance
- Dashboard — CLI + reporting is sufficient for 5-10 machines
- RBAC — single admin key is fine at this scale

### Stream 2: Enterprise Features (Licensed)

**Target:** Larger organizations (50+ machines) that need multi-team, multi-environment fleet management with compliance and audit requirements.

**Licensing model:** Open-core. The AGPL control plane is free. Enterprise features are available under a commercial license (per-CP-instance or per-managed-machine pricing).

**This stream is deferred until consulting validates the market.** The features below are envisioned but not committed.

## Envisioned Enterprise Features

### 1. Compliance Reporting Module

**Priority: HIGH — needed for consulting too.**

The CP already has audit events, machine registry, and health data. The compliance module structures this into reportable outputs.

**Reports:**
- **Machine inventory** — hostname, platform, current generation, last update, lifecycle state, tags
- **Drift detection** — desired vs actual generation per host. Machines out of sync are flagged.
- **Update latency** — time from "generation published" to "deployed fleet-wide." Shows deployment velocity.
- **Audit trail export** — all mutations (who did what, when) in CSV, JSON, or PDF format
- **Health summary** — aggregate health check results across the fleet. Percentage healthy over time.

**Rough API:**

```
GET /api/v1/reports/inventory              → machine inventory (JSON/CSV)
GET /api/v1/reports/drift                  → drift report
GET /api/v1/reports/update-latency         → deployment velocity stats
GET /api/v1/reports/health-summary         → aggregate health
GET /api/v1/audit/export?format=csv        → audit trail (already exists, needs CSV/PDF)
```

**NIS2 relevance:** NIS2 requires organizations to maintain asset inventories, demonstrate patch management, and provide audit trails. These reports map directly to NIS2 Article 21 requirements.

**Open questions:**
- PDF generation — Rust-native (printpdf) or delegate to external tool?
- Scheduled report generation (weekly email) or on-demand only?
- Dashboard UI or reports-only? (CLI + export might be sufficient initially)

### 2. Monitoring Integration

**Priority: MEDIUM — not needed for consulting MVP, but expected by enterprise.**

Not a full monitoring stack. Just the wiring to make nixfleet observable.

**Agent side:**
- Prometheus `node_exporter` module with sane defaults (enabled when agent is enabled)
- Agent exposes `/metrics` endpoint with: deploy count, rollback count, health check results, last poll time

**CP side:**
- CP exposes `/metrics` endpoint with: active rollouts, machine count by lifecycle state, health check success rate, API request latency
- Grafana dashboard template (JSON) for import

**Integration:**
- The CP metrics endpoint is useful standalone (Prometheus scrapes it)
- No Prometheus/Grafana deployment — that's fleet-specific infrastructure

### 3. Disk Encryption Enforcement

**Priority: MEDIUM — NIS2 compliance requirement.**

Framework-level eval checks that verify fleet-wide policies at evaluation time. If a policy is violated, `nix flake check` fails.

**Rough shape:**

```nix
nixfleet.compliance = {
  requireEncryption = true;     # eval fails if any host lacks LUKS/dm-crypt
  requireFirewall = true;       # eval fails if any host has firewall disabled
  requireImpermanence = false;  # optional policy
  maxStateVersion = "25.11";    # eval fails if any host has older stateVersion
};
```

**Implementation:** These are eval-time checks added to the test infrastructure. They inspect the NixOS configuration of each host and assert policy compliance. No runtime component.

**Open questions:**
- Should policies be per-tag-group or fleet-wide?
- How to handle exceptions (one host legitimately needs firewall disabled)?
- Where do policies live — flake.nix, a separate policies.nix, or CP-side?

### 4. Multi-Tenant Control Plane

**Priority: LOW — only for enterprise SaaS model.**

A single CP instance managing multiple isolated fleets (tenants). Each tenant has its own machines, rollouts, API keys, and audit trail. No cross-tenant visibility.

**Rough scope:**
- Tenant isolation in SQLite (tenant_id column on all tables) or PostgreSQL with schemas
- API key scoped to tenant
- Tenant CRUD (admin API)
- Resource limits per tenant (max machines, max rollouts)

**This is the biggest feature and should only be built when there are paying customers who need it.**

**Open questions:**
- SQLite per tenant (simple, file-based isolation) vs PostgreSQL with schemas (standard multi-tenant)?
- Does the agent need to know its tenant? (Probably not — the API key implies the tenant.)
- Tenant provisioning — self-service or admin-only?

### 5. RBAC (Role-Based Access Control)

**Priority: LOW — builds on multi-tenant.**

Current auth: API keys with roles (readonly/deploy/admin). Enterprise needs:
- Per-tag-group permissions ("user X can deploy to staging but not production")
- Audit of who can do what
- Integration with external identity providers (OIDC/SAML)

**Rough scope:**
- Roles table: `{role_name, permissions: JSON}`
- User-role assignment (or API-key-role)
- Permission check middleware
- OIDC integration (token validation against external IdP)

**Deferred until multi-tenant exists.** The current 3-tier role system (readonly/deploy/admin) is sufficient for Phase 3 and early consulting.

### 6. PostgreSQL Backend

**Priority: LOW — multi-tenant prerequisite.**

Current CP uses SQLite (WAL mode, single file). This is perfect for single-tenant, single-server deployment. Multi-tenant or high-availability requires PostgreSQL.

**Migration path:**
- Abstract DB layer behind a trait (currently all methods on `Db` struct)
- Implement PostgreSQL backend using `sqlx` or `tokio-postgres`
- Feature flag: `--db-backend sqlite|postgres`
- SQLite remains the default (zero-config)

### 7. Network Mesh (Aspirational)

**Priority: ASPIRATIONAL — far future.**

WireGuard mesh between fleet hosts, auto-configured from the machine registry.

```nix
nixfleet.mesh = {
  enable = true;
  # CP generates peer configs from machine registry
  # Agent-to-CP communication over WireGuard
  # Inter-machine communication over private IPs
};
```

The CP knows all machines and their public IPs. It could generate WireGuard configs and distribute them via the agent's desired-generation mechanism (or a separate config channel).

**This is the most ambitious feature and depends on everything else being stable.** It creates a private network overlay that simplifies firewall rules, enables direct machine-to-machine communication, and adds encryption in transit by default.

## Priority Summary

| Feature | Priority | Needed for | Depends on |
|---------|----------|-----------|------------|
| Compliance reporting | HIGH | Consulting MVP | Phase 1 (audit events exist) |
| Monitoring integration | MEDIUM | Enterprise readiness | Phase 3 (agent health exists) |
| Disk encryption enforcement | MEDIUM | NIS2 compliance | Phase 3 (eval test infra exists) |
| Multi-tenant CP | LOW | Enterprise SaaS | Consulting revenue validates market |
| RBAC | LOW | Enterprise multi-team | Multi-tenant CP |
| PostgreSQL backend | LOW | Multi-tenant CP | Multi-tenant design |
| Network mesh | ASPIRATIONAL | Zero-trust networking | Everything else stable |

## What NOT to Build

| Feature | Why not |
|---------|---------|
| Web dashboard | CLI + reports is sufficient. Dashboard is expensive to build and maintain. Revisit when there's clear demand. |
| Configuration management (Ansible replacement) | Nix IS the configuration management. Don't re-invent it. |
| Container orchestration | NixOS services + microVMs cover this. Don't compete with Kubernetes. |
| CI/CD | Use GitHub Actions / Forgejo. Not framework scope. |
| Secrets management solution | Agenix and sops-nix exist. Provide integration, not replacement. |

## Consulting Positioning

**NIS2 angle:**
- Article 21: "risk management measures" → nixfleet provides reproducible, auditable infrastructure
- Asset management → machine inventory + tag-based grouping
- Patch management → rollout system with canary/staged strategies
- Incident response → instant rollback, audit trail
- Supply chain security → Nix's hermetic builds, reproducible closures

**Pitch:**
> "Your infrastructure is a liability until it's auditable. NixFleet makes every machine declarative, every change tracked, and every deployment reversible. NIS2 compliance becomes a property of your system, not a quarterly checkbox."

**First 3 pilot engagements** validate the offering:
- 5-10 machines each
- Mix of bare metal and cloud
- Feedback → enterprise feature backlog
