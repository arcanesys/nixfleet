# Open Source Launch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make nixfleet discoverable and usable — flake templates work, docs are published, community announcements are drafted.

**Architecture:** Three code tasks (templates, docs workflow, README) + one content task (announcement drafts). All independent.

**Tech Stack:** Nix (flake templates), GitHub Actions (docs deployment), Markdown (announcements)

**Spec:** `docs/specs/2026-04-02-open-source-launch-design.md`

---

## File Map

### New files

| File | Responsibility |
|------|---------------|
| `.github/workflows/docs.yml` | GitHub Actions workflow for mdbook → GitHub Pages |
| `docs/community/discourse-announcement.md` | Draft NixOS Discourse post |
| `docs/community/nixcon-2026-proposal.md` | Draft NixCon talk proposal |

### Modified files

| File | Changes |
|------|---------|
| `modules/flake-module.nix` | Add `templates` output |
| `README.md` | Add Documentation section with links |

---

### Task 1: Flake Templates

**Files:**
- Modify: `modules/flake-module.nix`

- [ ] **Step 1: Read the current flake-module.nix**

Read `/home/s33d/dev/nix-org/nixfleet/modules/flake-module.nix` to understand the existing exports structure.

- [ ] **Step 2: Add templates output**

Add to the flake-module.nix, inside the config block alongside existing `flake.lib`, `flake.nixosModules`, etc.:

```nix
flake.templates = {
  standalone = {
    path = ../examples/standalone-host;
    description = "Single NixOS machine managed by NixFleet";
  };
  batch = {
    path = ../examples/batch-hosts;
    description = "Batch of identical hosts from a template";
  };
  fleet = {
    path = ../examples/client-fleet;
    description = "Multi-host fleet with flake-parts";
  };
  default = config.flake.templates.standalone;
};
```

Note: paths are relative to `modules/flake-module.nix`, so `../examples/` points to the repo root's `examples/` directory.

- [ ] **Step 3: Verify templates appear in flake show**

```bash
cd /home/s33d/dev/nix-org/nixfleet
nix flake show 2>&1 | grep -A5 templates
```

Expected output should list `standalone`, `batch`, `fleet`, and `default`.

- [ ] **Step 4: Test template init**

```bash
tmpdir=$(mktemp -d)
cd "$tmpdir"
nix flake init -t /home/s33d/dev/nix-org/nixfleet
ls -la
cat flake.nix
cd /home/s33d/dev/nix-org/nixfleet
rm -rf "$tmpdir"
```

Expected: `flake.nix` and other files from `examples/standalone-host/` appear in the temp directory.

- [ ] **Step 5: Format and verify**

```bash
nix fmt
nix flake check --no-build
```

- [ ] **Step 6: Commit**

```bash
git add modules/flake-module.nix
git commit -m "feat: expose examples as flake templates for nix flake init"
```

---

### Task 2: GitHub Pages Documentation Workflow

**Files:**
- Create: `.github/workflows/docs.yml`

- [ ] **Step 1: Create the workflow file**

Create `.github/workflows/docs.yml`:

```yaml
name: Deploy Documentation

on:
  push:
    branches: [main]
    paths:
      - "docs/src/**"
      - "docs/src/book.toml"
  workflow_dispatch:

permissions:
  pages: write
  id-token: write

concurrency:
  group: pages
  cancel-in-progress: true

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: cachix/install-nix-action@v27
        with:
          nix_path: nixpkgs=channel:nixos-unstable

      - name: Build mdbook
        run: nix develop --command mdbook build docs/src

      - uses: actions/upload-pages-artifact@v3
        with:
          path: result-docs

  deploy:
    needs: build
    runs-on: ubuntu-latest
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    steps:
      - name: Deploy to GitHub Pages
        id: deployment
        uses: actions/deploy-pages@v4
```

- [ ] **Step 2: Verify the mdbook builds locally**

```bash
cd /home/s33d/dev/nix-org/nixfleet
nix develop --command mdbook build docs/src
ls result-docs/index.html
```

Expected: `result-docs/` contains the built HTML site.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/docs.yml
git commit -m "ci: add GitHub Pages workflow for mdbook documentation"
```

---

### Task 3: README Documentation Section

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add Documentation section to README**

After the "Quick Start" section and before "Layout", add:

```markdown
## Documentation

Full documentation: [your-org.github.io/nixfleet](https://your-org.github.io/nixfleet/)

- [Quick Start](https://your-org.github.io/nixfleet/guide/getting-started/quick-start.html) — first host in 15 minutes
- [Concepts](https://your-org.github.io/nixfleet/guide/concepts/) — architecture, scopes, hostSpec
- [Reference](https://your-org.github.io/nixfleet/reference/) — mkHost API, deployment commands
- [Fleet Orchestration](docs/specs/2026-04-02-fleet-orchestration-design.md) — tags, health checks, rollout strategies
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: add documentation section to README with published docs links"
```

---

### Task 4: Community Announcement Drafts

**Files:**
- Create: `docs/community/discourse-announcement.md`
- Create: `docs/community/nixcon-2026-proposal.md`

- [ ] **Step 1: Create discourse announcement draft**

Create `docs/community/discourse-announcement.md`:

```markdown
# NixFleet — Declarative NixOS Fleet Management with Staged Rollouts

*Draft for NixOS Discourse — post when repo is public*

---

**NixFleet** is a framework for managing fleets of NixOS and macOS machines. It combines a Nix module system for declarative configuration with a Rust-based control plane for fleet orchestration.

## What makes it different?

Most NixOS deployment tools (colmena, deploy-rs, morph) focus on **pushing configurations to machines**. NixFleet goes further:

- **Staged rollouts** — canary, percentage-based, and all-at-once strategies with automatic pause on failure
- **Health checks** — declarative systemd, HTTP, and command checks that determine whether a deployment succeeded
- **Automatic rollback** — agents self-rollback if health checks fail; the control plane reverts fleet-wide if a rollout batch exceeds the failure threshold
- **Persistent state** — the control plane tracks machine inventory, deployment history, and audit events in SQLite
- **Standard tooling** — no custom deployment scripts. `nixos-rebuild`, `nixos-anywhere`, and `darwin-rebuild` work as-is

The framework itself is a single function: `mkHost`. It takes a hostname, platform, and optional configuration flags, and returns a standard `nixosSystem` or `darwinSystem`. No DSL to learn.

## Quick Start

```sh
mkdir my-fleet && cd my-fleet
nix flake init -t nixfleet
# Edit flake.nix with your host details
nixos-anywhere --flake .#myhost root@192.168.1.50
```

## Architecture

```
Fleet repo (your config)
    ↓ consumes
NixFleet framework (mkHost + core modules + scopes)
    ↓ builds
NixOS/Darwin systems
    ↓ deployed via
Agent (per machine) ↔ Control Plane (orchestrator)
```

The agent is a Rust binary that polls the control plane, applies generations, runs health checks, and reports status. The control plane orchestrates rollouts, tracks machine state, and provides a REST API.

## Links

- **Repository:** https://github.com/your-org/nixfleet
- **Documentation:** https://your-org.github.io/nixfleet/
- **Design decisions:** See `docs/decisions/` in the repo
- **Fleet orchestration spec:** See `docs/specs/2026-04-02-fleet-orchestration-design.md`

## Status

The framework and orchestration layer are functional. We're looking for early adopters and feedback. If you manage 3+ NixOS machines and want to try it, we'd love to hear from you.

Feedback welcome here or as GitHub issues.
```

- [ ] **Step 2: Create NixCon proposal draft**

Create `docs/community/nixcon-2026-proposal.md`:

```markdown
# NixCon 2026 Talk Proposal

*Draft — submit when CFP opens*

---

## Title

Managing NixOS Fleets at Scale — From mkHost to Canary Deployments

## Format

Talk (30 minutes) or Workshop (2 hours)

## Abstract

Managing a single NixOS machine is a solved problem. Managing ten is repetitive. Managing fifty without drift, with safe rollouts, and with compliance reporting — that's where NixFleet comes in.

This talk introduces NixFleet, an open-source framework that combines declarative Nix configuration with a Rust-based control plane for fleet orchestration. We'll cover:

1. **The framework** — a single `mkHost` function that replaces boilerplate with flags. No DSL, no ceremony. Standard `nixos-rebuild` and `nixos-anywhere` work out of the box.

2. **The orchestration layer** — agents on each machine poll a control plane, apply generations, run health checks, and report status. The control plane drives staged rollouts (canary → percentage → all-at-once) with automatic pause and rollback on failure.

3. **Live demo** — define two hosts, deploy them, trigger a canary rollout with an intentionally broken generation, watch the health checks fail, and see the automatic rollback.

4. **Architecture decisions** — why a single function over a DSL, why hostSpec flags over roles, why the agent stays simple and the control plane stays smart.

5. **What's next** — binary cache integration, microVM support, NIS2 compliance reporting.

## Target Audience

NixOS users managing more than one machine. DevOps engineers evaluating Nix for fleet management. Anyone interested in declarative infrastructure at scale.

## Speaker Bio

[To be filled]

## Notes for Reviewers

NixFleet is open-source (AGPL control plane, MIT framework). The live demo uses QEMU VMs — no external infrastructure needed. All code shown will be available in the public repository.
```

- [ ] **Step 3: Commit**

```bash
mkdir -p docs/community
git add docs/community/discourse-announcement.md docs/community/nixcon-2026-proposal.md
git commit -m "docs: draft community announcements for Discourse and NixCon 2026"
```

---

### Task 5: Final Verification

- [ ] **Step 1: Verify flake templates work**

```bash
cd /home/s33d/dev/nix-org/nixfleet
nix flake show 2>&1 | grep -A10 templates
```

- [ ] **Step 2: Verify mdbook builds**

```bash
nix develop --command mdbook build docs/src
test -f result-docs/index.html && echo "OK" || echo "FAIL"
```

- [ ] **Step 3: Verify all tests pass**

```bash
cargo test --workspace
nix flake check --no-build
```

- [ ] **Step 4: Review commit log**

```bash
git log --oneline HEAD~5..HEAD
```

Expected: 4-5 clean commits covering templates, docs workflow, README, announcements, and any fixes.
