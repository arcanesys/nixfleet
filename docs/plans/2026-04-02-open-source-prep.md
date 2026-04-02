# Open Source Preparation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prepare the nixfleet repository for public release — licensing, cleanup, anonymization, contributor docs.

**Architecture:** Pure docs/config changes. No code logic changes except one CLI default value. Six sequential tasks: licensing → internal docs removal → anonymization → README update → contributor docs → final verification.

**Tech Stack:** Markdown, TOML, Rust (one default value), Nix (formatting)

**Spec:** `docs/specs/2026-04-02-open-source-prep-design.md`

---

## File Map

### New files

| File | Responsibility |
|------|---------------|
| `LICENSE-MIT` | MIT license text (framework, agent, CLI, shared) |
| `LICENSE-AGPL` | AGPL-3.0 license text (control plane) |
| `control-plane/LICENSE` | AGPL-3.0 license for the control-plane crate |
| `CONTRIBUTING.md` | Contributor guidelines |
| `SECURITY.md` | Vulnerability reporting process |

### Modified files

| File | Changes |
|------|---------|
| `README.md` | Replace abstracts33d URLs, update license section, remove private repo links |
| `CLAUDE.md` | Replace abstracts33d refs, remove private repo references |
| `cli/src/main.rs` | Change default --org from `abstracts33d` to `my-org` |
| `docs/src/book.toml` | Author → `nixfleet contributors`, repo URL → placeholder |
| `docs/src/guide/getting-started/quick-start.md` | Example URLs → placeholder |
| `examples/standalone-host/flake.nix` | Input URL → placeholder |
| `agent/Cargo.toml` | License → MIT, add repository/homepage/authors |
| `cli/Cargo.toml` | License → MIT, add repository/homepage/authors |
| `shared/Cargo.toml` | License → MIT, add repository/homepage/authors |
| `control-plane/Cargo.toml` | License → AGPL-3.0-only, add repository/homepage/authors |

### Deleted files

| Path | Reason |
|------|--------|
| `LICENSE` | Replaced by LICENSE-MIT + LICENSE-AGPL |
| `docs/superpowers/` (entire directory) | Internal GTM strategy and planning docs |
| `docs/plans/2026-04-02-fleet-orchestration.md` | Internal implementation plan |

---

### Task 1: Dual Licensing

**Files:**
- Delete: `LICENSE`
- Create: `LICENSE-MIT`
- Create: `LICENSE-AGPL`
- Create: `control-plane/LICENSE`
- Modify: `agent/Cargo.toml`
- Modify: `cli/Cargo.toml`
- Modify: `shared/Cargo.toml`
- Modify: `control-plane/Cargo.toml`

- [ ] **Step 1: Delete current Apache-2.0 license**

```bash
cd /home/s33d/dev/nix-org/nixfleet
git rm LICENSE
```

- [ ] **Step 2: Create LICENSE-MIT**

Create `LICENSE-MIT` with the full MIT license text:

```
MIT License

Copyright (c) 2025-present nixfleet contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

- [ ] **Step 3: Create LICENSE-AGPL**

Create `LICENSE-AGPL` with the full AGPL-3.0 text. Download from https://www.gnu.org/licenses/agpl-3.0.txt or use the standard text. The file should start with:

```
GNU AFFERO GENERAL PUBLIC LICENSE
Version 3, 19 November 2007

Copyright (C) 2007 Free Software Foundation, Inc. <https://fsf.org/>
...
```

Add a copyright header at the top before the license text:

```
nixfleet control plane
Copyright (c) 2025-present nixfleet contributors

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU Affero General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.
```

Then the full AGPL-3.0 text.

- [ ] **Step 4: Create control-plane/LICENSE**

Create `control-plane/LICENSE` with content:

```
The nixfleet control plane is licensed under the GNU Affero General Public
License version 3.0 (AGPL-3.0).

See ../LICENSE-AGPL for the full license text.

For commercial licensing inquiries, see the project README.
```

- [ ] **Step 5: Update Cargo.toml metadata**

In `shared/Cargo.toml`, `agent/Cargo.toml`, and `cli/Cargo.toml`, change:
```toml
license = "MIT"
repository = "https://github.com/your-org/nixfleet"
homepage = "https://github.com/your-org/nixfleet"
authors = ["nixfleet contributors"]
```

In `control-plane/Cargo.toml`, change:
```toml
license = "AGPL-3.0-only"
repository = "https://github.com/your-org/nixfleet"
homepage = "https://github.com/your-org/nixfleet"
authors = ["nixfleet contributors"]
```

- [ ] **Step 6: Verify build**

```bash
cargo test --workspace
```

Expected: All tests pass (license metadata doesn't affect compilation).

- [ ] **Step 7: Commit**

```bash
git add LICENSE-MIT LICENSE-AGPL control-plane/LICENSE agent/Cargo.toml cli/Cargo.toml shared/Cargo.toml control-plane/Cargo.toml
git commit -m "chore: switch to dual license — AGPL-3.0 (control plane) + MIT (framework, agent, CLI)"
```

---

### Task 2: Remove Internal Documentation

**Files:**
- Delete: `docs/superpowers/` (entire directory)
- Delete: `docs/plans/2026-04-02-fleet-orchestration.md`

- [ ] **Step 1: Remove internal superpowers docs**

```bash
cd /home/s33d/dev/nix-org/nixfleet
git rm -r docs/superpowers/
```

- [ ] **Step 2: Remove internal implementation plan**

```bash
git rm docs/plans/2026-04-02-fleet-orchestration.md
```

Note: Keep `docs/specs/2026-04-02-fleet-orchestration-design.md` (public design doc) and `docs/specs/2026-04-02-open-source-prep-design.md` (remove after this plan is complete).

- [ ] **Step 3: Commit**

```bash
git commit -m "chore: remove internal planning and GTM documentation"
```

---

### Task 3: Anonymize Personal References

**Files:**
- Modify: `cli/src/main.rs`
- Modify: `CLAUDE.md`
- Modify: `docs/src/book.toml`
- Modify: `docs/src/guide/getting-started/quick-start.md`
- Modify: `examples/standalone-host/flake.nix`

- [ ] **Step 1: Fix CLI default org**

In `cli/src/main.rs`, find line 135:
```rust
        #[arg(long, default_value = "abstracts33d")]
```
Change to:
```rust
        #[arg(long, default_value = "my-org")]
```

- [ ] **Step 2: Fix docs/src/book.toml**

Replace full content with:
```toml
[book]
title = "NixFleet Documentation"
authors = ["nixfleet contributors"]
language = "en"
src = "."

[build]
build-dir = "../../result-docs"

[output.html]
git-repository-url = "https://github.com/your-org/nixfleet"
```

- [ ] **Step 3: Fix quick-start.md**

In `docs/src/guide/getting-started/quick-start.md`:

Line 8 — change:
```markdown
- A fleet repository consuming NixFleet (see [examples/](https://github.com/abstracts33d/nixfleet/tree/main/examples))
```
To:
```markdown
- A fleet repository consuming NixFleet (see [examples/](https://github.com/your-org/nixfleet/tree/main/examples))
```

Line 22 — change:
```nix
  inputs.nixfleet.url = "github:abstracts33d/nixfleet";
```
To:
```nix
  inputs.nixfleet.url = "github:your-org/nixfleet";
```

- [ ] **Step 4: Fix standalone example**

In `examples/standalone-host/flake.nix`, line 11 — change:
```nix
    nixfleet.url = "github:abstracts33d/nixfleet";
```
To:
```nix
    nixfleet.url = "github:your-org/nixfleet";
```

- [ ] **Step 5: Scrub CLAUDE.md**

In `CLAUDE.md`:

Line 105 — change:
```nix
  inputs.nixfleet.url = "github:abstracts33d/nixfleet";
```
To:
```nix
  inputs.nixfleet.url = "github:your-org/nixfleet";
```

Lines 133-134 — replace the Related Repos table:
```markdown
| [fleet](https://github.com/abstracts33d/fleet) | Reference fleet (abstracts33d org config, hardware, dotfiles) |
| [fleet-secrets](https://github.com/abstracts33d/fleet-secrets) | Encrypted secrets (agenix) |
```
With:
```markdown
| your fleet repo | Your org's fleet configuration consuming nixfleet |
```

- [ ] **Step 6: Check for any remaining references**

Run:
```bash
grep -rn "abstracts33d\|abstract\.s33d\|/home/s33d" --include='*.rs' --include='*.nix' --include='*.md' --include='*.toml' --include='*.yml' .
```

Expected: Zero results (excluding `.git/` and the open-source-prep spec/plan which will be removed).

If any results remain, fix them. The fleet-orchestration design spec (`docs/specs/2026-04-02-fleet-orchestration-design.md`) should NOT contain personal references — if it does, fix it.

- [ ] **Step 7: Format and verify**

```bash
nix fmt
cargo test --workspace
```

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "chore: anonymize personal references — replace abstracts33d with your-org placeholder"
```

---

### Task 4: Update README

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Replace the README**

The README needs three changes:
1. Quick Start URL: `github:abstracts33d/nixfleet` → `github:your-org/nixfleet`
2. Related Repos section: remove private repo links
3. License section: update for dual license

Replace the full README with:

```markdown
# NixFleet

**Declarative NixOS fleet management.** Define your infrastructure as code with reproducible builds, instant rollback, and zero config drift.

## What is NixFleet?

NixFleet is a framework for managing fleets of NixOS and macOS machines. It provides:
- **`mkHost`** — single function that returns a standard `nixosSystem` or `darwinSystem`
- **hostSpec** — extensible host configuration flags (fleet repos add their own)
- **Core modules** — nix settings, boot, SSH hardening, networking, user management
- **Disko templates** — reusable disk layout configurations
- **Agent + Control Plane** — Rust-based fleet orchestration with staged rollouts, health checks, and automatic rollback

## Quick Start

```nix
# flake.nix — single machine, no ceremony
{
  inputs.nixfleet.url = "github:your-org/nixfleet";
  inputs.nixpkgs.follows = "nixfleet/nixpkgs";

  outputs = {nixfleet, ...}: {
    nixosConfigurations.myhost = nixfleet.lib.mkHost {
      hostName = "myhost";
      platform = "x86_64-linux";
      hostSpec = {
        userName = "alice";
        timeZone = "US/Eastern";
        sshAuthorizedKeys = [ "ssh-ed25519 AAAA..." ];
      };
      modules = [
        ./hardware-configuration.nix
        ./disk-config.nix
      ];
    };
  };
}
```

Deploy:
```sh
nixos-anywhere --flake .#myhost root@192.168.1.50   # fresh install
sudo nixos-rebuild switch --flake .#myhost           # rebuild
```

See `examples/` for more patterns (standalone host, batch hosts, client fleet).

## Layout

```
modules/
├── _shared/lib/       # Framework API: mkHost, mkVmApps
├── _shared/           # hostSpec options, disk templates
├── core/              # Core NixOS/Darwin modules
├── scopes/            # Scope modules (base, impermanence, agent, control-plane)
├── tests/             # Eval tests, VM tests, integration tests
├── apps.nix           # Flake apps (validate, VM helpers)
├── fleet.nix          # Test fleet for framework CI
└── flake-module.nix   # Framework exports
examples/
├── standalone-host/   # Single machine in its own repo
├── batch-hosts/       # 50 edge devices from a template
└── client-fleet/      # Fleet consuming mkHost via flake-parts
```

## Scope Pattern

mkHost auto-includes framework scopes. They self-activate based on hostSpec flags:

```nix
# isImpermanent = true -> impermanence scope activates (btrfs wipe, persistence paths)
# isMinimal = true -> base scope skips optional packages
# services.nixfleet-agent.enable = true -> agent service starts
```

Fleet repos add their own scopes (catppuccin, hyprland, dev tools, etc.) as plain NixOS/HM modules.

## Fleet Orchestration

The agent + control plane provide fleet-wide deployment orchestration:

- **Machine tags** — group machines for targeted operations
- **Health checks** — declarative systemd, HTTP, and command checks
- **Rollout strategies** — canary, staged, all-at-once with automatic pause/revert
- **CLI** — `nixfleet deploy --tag production --strategy canary --wait`

See `docs/specs/2026-04-02-fleet-orchestration-design.md` for the full design.

## Deployment

Standard NixOS tooling — no custom scripts:

```sh
nixos-anywhere --flake .#hostname root@ip              # fresh install (formats disks via disko)
sudo nixos-rebuild switch --flake .#hostname            # local rebuild
nixos-rebuild switch --flake .#hostname --target-host root@ip  # remote rebuild
darwin-rebuild switch --flake .#hostname                # macOS
```

## Virtual Machines

```sh
nix run .#spawn-qemu -- --iso iso/nixos-x86_64.iso   # first boot from ISO
nix run .#spawn-qemu                                   # subsequent boots
nix run .#spawn-qemu -- --persistent -h web-02         # build + install + launch (graphical)
nix run .#test-vm -- -h web-02                         # full VM test cycle
```

Fleet repos wire these via `nixfleet.lib.mkVmApps { inherit pkgs; }`.

## Development

```sh
nix develop                        # dev shell
nix flake check --no-build         # eval tests (instant)
nix run .#validate                 # full validation (eval + host builds)
nix run .#validate -- --vm         # include VM tests
nix fmt                            # format (alejandra + shfmt)
cargo test --workspace             # Rust tests
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for detailed contributor guidelines.

## License

nixfleet uses a dual-license model:

- **Control Plane** (`control-plane/`): [AGPL-3.0](LICENSE-AGPL) — modifications to the control plane must be shared when provided as a service
- **Everything else** (framework, agent, CLI, modules): [MIT](LICENSE-MIT) — use freely, no copyleft obligation

This means you can freely use the framework to manage your fleet without any copyleft requirement. Your fleet configurations, custom modules, and agent deployments remain fully private.
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: update README for open source — dual license, fleet orchestration section, remove private links"
```

---

### Task 5: Contributor Documentation

**Files:**
- Create: `CONTRIBUTING.md`
- Create: `SECURITY.md`

- [ ] **Step 1: Create CONTRIBUTING.md**

```markdown
# Contributing to NixFleet

Thank you for your interest in contributing to NixFleet!

## Development Setup

**Prerequisites:**
- [Nix](https://nixos.org/download) with flakes enabled (`experimental-features = nix-command flakes`)

**Getting started:**
```sh
git clone https://github.com/your-org/nixfleet.git
cd nixfleet
nix develop  # enters the dev shell with Rust toolchain, cargo, rustfmt, clippy
```

## Project Structure

```
agent/          # Fleet agent (Rust) — runs on each managed machine
control-plane/  # Control plane server (Rust) — orchestrates deployments
cli/            # Operator CLI (Rust) — manages the fleet
shared/         # Shared types (Rust) — API contracts between crates
modules/        # Nix modules — framework core, scopes, tests
examples/       # Consumption patterns — standalone, batch, client fleet
docs/           # Documentation — mdbook source
```

## Running Tests

```sh
# Rust tests (fast, run frequently)
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check

# Nix eval tests (instant, no builds)
nix flake check --no-build

# Full validation (slow, builds all hosts)
nix run .#validate

# VM tests (slowest, boots VMs)
nix run .#validate -- --vm
```

## Code Style

- **Rust:** `rustfmt` defaults. Run `cargo fmt --all` before committing.
- **Nix:** `alejandra` formatter. Run `nix fmt` before committing.
- Pre-commit hooks enforce both automatically.

## Commit Conventions

Use [conventional commits](https://www.conventionalcommits.org/):

- `feat:` — new feature
- `fix:` — bug fix
- `docs:` — documentation only
- `chore:` — maintenance, dependencies, CI
- `test:` — test additions or fixes
- `refactor:` — code restructuring without behavior change

Keep the subject line under 72 characters. Use the body to explain "why," not "what."

## Pull Requests

1. Create a feature branch: `feat/`, `fix/`, `docs/`, etc.
2. Make your changes with tests
3. Ensure all checks pass (`cargo test --workspace && cargo clippy --workspace -- -D warnings && nix flake check --no-build`)
4. Open a PR with a clear description of the change and its motivation
5. One feature per PR — keep changes focused

## Architecture Decisions

Significant design decisions are recorded in `docs/decisions/` as Architecture Decision Records (ADRs). Read the existing ADRs before proposing changes to core patterns like `mkHost`, hostSpec, or the agent state machine.

## License

By submitting a pull request, you agree to license your contribution under the project's applicable license:

- Contributions to `control-plane/` are licensed under **AGPL-3.0** ([LICENSE-AGPL](LICENSE-AGPL))
- Contributions to all other directories are licensed under **MIT** ([LICENSE-MIT](LICENSE-MIT))
```

- [ ] **Step 2: Create SECURITY.md**

```markdown
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
```

- [ ] **Step 3: Commit**

```bash
git add CONTRIBUTING.md SECURITY.md
git commit -m "docs: add CONTRIBUTING.md and SECURITY.md for open source"
```

---

### Task 6: Final Verification and Cleanup

**Files:** None new — verification only

- [ ] **Step 1: Verify no personal references remain**

```bash
cd /home/s33d/dev/nix-org/nixfleet
grep -rn "abstracts33d\|abstract\.s33d\|/home/s33d" --include='*.rs' --include='*.nix' --include='*.md' --include='*.toml' --include='*.yml' . | grep -v "\.git/" | grep -v "open-source-prep"
```

Expected: Zero results. If any remain, fix them.

- [ ] **Step 2: Verify Rust builds and tests pass**

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

Expected: All pass.

- [ ] **Step 3: Verify Nix formatting and eval**

```bash
nix fmt
nix flake check --no-build
```

Expected: All pass.

- [ ] **Step 4: Remove the open-source-prep spec and plan (self-referential docs)**

```bash
git rm docs/specs/2026-04-02-open-source-prep-design.md
git rm docs/plans/2026-04-02-open-source-prep.md
```

- [ ] **Step 5: Final commit**

```bash
git add -A
git commit -m "chore: final open-source prep cleanup"
```

- [ ] **Step 6: Review commit log**

```bash
git log --oneline HEAD~6..HEAD
```

Expected: 6 clean commits covering licensing, cleanup, anonymization, README, contributor docs, and final verification.
