# Open Source Preparation — Repo Hygiene

**Date:** 2026-04-02
**Status:** Draft
**Scope:** Licensing, cleanup, anonymization, contributor docs — everything needed before making the repo public
**Prerequisite:** PR #7 (fleet orchestration) merged to main

## Overview

Prepare the nixfleet repository for public release. This spec covers repo hygiene only — flake templates, docs deployment, landing page, and community launch are deferred to Spec B.

The goal: a stranger visiting the repo sees a professional, well-documented open-source project with clear licensing, no personal data leakage, and contributor guidelines.

## 1. Licensing

**Current:** Apache-2.0 everywhere.
**Target:** AGPL-3.0 (control-plane) + MIT (framework, agent, CLI, shared types, Nix modules).

**Rationale:** The control plane is the commercial moat for Phase 4 enterprise features (multi-tenant, RBAC, compliance reporting). AGPL ensures anyone running a modified CP as a service must share their modifications. MIT on everything else maximizes framework adoption — fleet repos and agents have zero copyleft obligation.

### Files

- Remove: `LICENSE` (current Apache-2.0)
- Create: `LICENSE-MIT` at repo root — full MIT text, copyright `nixfleet contributors`
- Create: `LICENSE-AGPL` at repo root — full AGPL-3.0 text
- Create: `control-plane/LICENSE` — symlink or copy of `LICENSE-AGPL` with header noting this crate is AGPL

### Cargo.toml Updates

| Crate | License field |
|-------|--------------|
| `shared` (nixfleet-types) | `MIT` |
| `agent` | `MIT` |
| `cli` | `MIT` |
| `control-plane` | `AGPL-3.0-only` |

All Cargo.toml files also gain:
```toml
repository = "https://github.com/your-org/nixfleet"
homepage = "https://github.com/your-org/nixfleet"
authors = ["nixfleet contributors"]
```

(`your-org` is a placeholder until the GitHub org is decided.)

### README License Section

Add a "License" section to README.md:

```markdown
## License

nixfleet uses a dual-license model:

- **Control Plane** (`control-plane/`): [AGPL-3.0](LICENSE-AGPL)
- **Everything else** (framework, agent, CLI, modules): [MIT](LICENSE-MIT)

This means you can freely use the framework to manage your fleet without any copyleft obligation. The AGPL on the control plane ensures that anyone providing a modified control plane as a service shares their modifications.

For commercial licensing of the control plane (e.g., proprietary modifications), contact [TBD].
```

## 2. Repo Cleanup

### Remove Internal Documentation

These files contain business strategy (GTM), personal data, and internal implementation plans that should not be public:

```
git rm -r docs/superpowers/
git rm docs/plans/2026-04-02-fleet-orchestration.md
```

Keep `docs/specs/2026-04-02-fleet-orchestration-design.md` — the design spec is useful public documentation of the rollout system's architecture.

Keep `docs/specs/2026-04-02-open-source-prep-design.md` (this file) — remove after implementation if desired.

### Scrub CLAUDE.md

Keep the file (useful for AI-assisted contributors). Remove:
- All `abstracts33d` references → `your-org`
- References to private repos (`fleet`, `fleet-secrets`) → generic examples
- Any personal paths (`/home/s33d/`)
- Personal email addresses

### Anonymize Hardcoded References

Every occurrence of `abstracts33d`, `abstract.s33d@gmail.com`, and `/home/s33d/` must be replaced or removed.

| File | What to change |
|------|---------------|
| `cli/src/main.rs` | Default `--org` value: `abstracts33d` → `my-org` |
| `README.md` | All `github:abstracts33d/nixfleet` → `github:your-org/nixfleet` |
| `README.md` | Remove links to private repos (`fleet`, `fleet-secrets`) from Related Projects |
| `CLAUDE.md` | All `abstracts33d` → `your-org`, remove private repo references |
| `docs/src/book.toml` | Author → `nixfleet contributors`, repo URL → placeholder |
| `docs/src/guide/getting-started/quick-start.md` | Example URLs → placeholder |
| `examples/standalone-host/flake.nix` | `github:abstracts33d/nixfleet` → `github:your-org/nixfleet` |
| `examples/client-fleet/flake.nix` | Same |
| `examples/batch-hosts/flake.nix` | Same |
| Any other file found by `grep -r abstracts33d` | Replace accordingly |

### Verify No Personal Data Remains

After all replacements, run:
```bash
grep -r "abstracts33d\|abstract\.s33d\|/home/s33d" --include='*.rs' --include='*.nix' --include='*.md' --include='*.toml' --include='*.yml'
```

Must return zero results (excluding `.git/` directory).

## 3. CONTRIBUTING.md

Create `CONTRIBUTING.md` at repo root:

### Content

**Development Setup:**
- Prerequisites: Nix with flakes enabled
- `nix develop` for the dev shell
- Rust workspace in `agent/`, `control-plane/`, `cli/`, `shared/`
- Nix modules in `modules/`

**Running Tests:**
- `cargo test --workspace` — Rust unit + integration tests
- `cargo clippy --workspace -- -D warnings` — lint
- `cargo fmt --all -- --check` — formatting
- `nix flake check --no-build` — Nix eval tests
- `nix run .#validate` — full validation (eval + host builds)
- `nix run .#validate -- --vm` — VM tests (slow, optional)

**Code Style:**
- Rust: `rustfmt` defaults
- Nix: `alejandra` formatter
- Commits: conventional commits (`feat:`, `fix:`, `docs:`, `chore:`, `test:`, `refactor:`)

**Pull Request Process:**
- Create a feature branch (`feat/`, `fix/`, etc.)
- Ensure all tests pass
- One feature per PR
- Description explains the "why"

**License:**
- Contributions to `control-plane/` are licensed under AGPL-3.0
- Contributions to all other crates and modules are licensed under MIT
- By submitting a PR, you agree to license your contribution under the applicable license

## 4. SECURITY.md

Create `SECURITY.md` at repo root:

### Content

**Reporting Vulnerabilities:**
- Do NOT open a public issue
- Use GitHub's private security advisory feature (Security tab → Report a vulnerability)
- Or email [TBD security contact]

**Scope:**
- Control plane authentication and authorization
- Agent-to-CP communication security (mTLS, API keys)
- Secret handling in Nix modules
- SQLite injection or data exposure

**Response:**
- Acknowledge within 48 hours
- Assess severity within 1 week
- Fix critical issues within 2 weeks
- Coordinate disclosure timeline with reporter

## 5. Notes for Spec B (Deferred)

These items are out of scope for this spec but documented for the next phase:

- **Flake templates:** Expose `examples/` as `templates` output in flake.nix so `nix flake init -t nixfleet#standalone` works
- **GitHub Pages:** Add workflow to build and deploy mdbook docs to GitHub Pages
- **Landing page:** Create `docs/landing/` or use GitHub Pages index with project overview
- **Community posts:** NixOS Discourse announcement, Hacker News
- **NixCon 2026:** Talk proposal for fleet management with Nix
- **Org decision:** Replace all `your-org` placeholders with the real GitHub org name
- **Commercial contact:** Replace license TBD with actual contact for commercial licensing
