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
docs/           # ADRs and mdbook documentation
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

Significant design decisions are recorded in `docs/adr/` as Architecture Decision Records (ADRs). Read the existing ADRs before proposing changes to core patterns like `mkHost`, hostSpec, or the agent state machine.

## License

By submitting a pull request, you agree to license your contribution under the project's applicable license:

- Contributions to `control-plane/` are licensed under **AGPL-3.0** ([LICENSE-AGPL](LICENSE-AGPL))
- Contributions to all other directories are licensed under **MIT** ([LICENSE-MIT](LICENSE-MIT))
