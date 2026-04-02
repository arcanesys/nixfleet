# Open Source Launch — Community & Developer Experience

**Date:** 2026-04-02
**Status:** Draft
**Scope:** Flake templates, docs deployment, community launch — everything after the repo is public
**Prerequisite:** Phase 2A (repo preparation) complete and repo is public

## Overview

Make the "stranger can use nixfleet in 15 minutes" promise real. Spec A cleaned the repo. This spec makes it discoverable and usable.

## 1. Flake Templates

**Problem:** `nix flake init -t nixfleet` doesn't work. The flake.nix has no `templates` output. Examples exist in `examples/` but aren't exposed as templates.

**Solution:** Add a `templates` output to `flake-module.nix` exposing the three existing example patterns.

```nix
flake.templates = {
  standalone = {
    path = ./examples/standalone-host;
    description = "Single NixOS machine managed by NixFleet";
  };
  batch = {
    path = ./examples/batch-hosts;
    description = "Batch of identical hosts from a template";
  };
  fleet = {
    path = ./examples/client-fleet;
    description = "Multi-host fleet with flake-parts";
  };
  default = self.templates.standalone;
};
```

**Usage after:**

```sh
mkdir my-fleet && cd my-fleet
nix flake init -t nixfleet              # standalone (default)
nix flake init -t nixfleet#fleet        # full fleet with flake-parts
nix flake init -t nixfleet#batch        # batch hosts
```

**Verification:** `nix flake show` lists templates. `nix flake init -t .` works from a temp directory.

## 2. GitHub Pages — mdbook Deployment

**Problem:** Documentation exists as mdbook source in `docs/src/` but isn't published anywhere. A stranger has to clone the repo and build locally.

**Solution:** GitHub Actions workflow that builds mdbook and deploys to GitHub Pages on push to main.

**Workflow:** `.github/workflows/docs.yml`

```yaml
name: Deploy Documentation
on:
  push:
    branches: [main]
    paths: [docs/src/**]
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
      - run: nix develop --command mdbook build docs/src
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
      - id: deployment
        uses: actions/deploy-pages@v4
```

**Result:** Docs live at `https://your-org.github.io/nixfleet/` (or custom domain later).

## 3. README Enhancement

Add a "Documentation" section to README.md linking to the published docs:

```markdown
## Documentation

Full documentation at [your-org.github.io/nixfleet](https://your-org.github.io/nixfleet/).

- [Quick Start](https://your-org.github.io/nixfleet/guide/getting-started/quick-start.html)
- [Architecture](https://your-org.github.io/nixfleet/guide/concepts/architecture.html)
- [API Reference](https://your-org.github.io/nixfleet/reference/)
```

## 4. Community Announcement (Content Only)

Prepare announcement drafts. Not automated — posted manually when ready.

### NixOS Discourse Post

Target: `https://discourse.nixos.org/c/announcements/`

**Title:** "NixFleet — Declarative NixOS fleet management with staged rollouts"

**Content structure:**
- What it is (1 paragraph)
- What makes it different from existing tools (colmena, deploy-rs, morph)
- Quick start (3 commands)
- Architecture overview (mkHost, agent, CP, rollouts)
- Link to repo + docs
- Call for feedback

**Differentiators vs existing tools:**
- Not just deployment — full lifecycle (health checks, staged rollouts, automatic rollback)
- Rust control plane with persistent state (not a one-shot deploy script)
- Single `mkHost` API (no DSL to learn)
- Standard NixOS commands (`nixos-rebuild`, `nixos-anywhere`) — no custom deployment ceremony

### Hacker News Post

Short title: "Show HN: NixFleet – Declarative NixOS fleet management with canary deployments"

Link to repo. Let the README speak.

### NixCon 2026 Talk Proposal

**Title:** "Managing NixOS Fleets at Scale — From mkHost to Canary Deployments"

**Abstract structure:**
- The problem: managing N NixOS machines without drift
- The approach: declarative framework + Rust orchestration
- Demo: define a host, deploy, trigger a canary rollout, watch health checks, automatic rollback
- Architecture: mkHost, hostSpec, scopes, agent state machine, CP rollout executor
- What's next: Attic integration, microVMs, compliance reporting

## 5. Notes for Later (Out of Scope)

- Custom domain for docs (requires DNS setup)
- Landing page with marketing copy (separate from mdbook technical docs)
- Logo / branding
- Replace `your-org` placeholders with real org name (blocked on org decision)
