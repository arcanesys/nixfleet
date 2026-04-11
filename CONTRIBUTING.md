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

**One command runs everything:**

```sh
nix run .#validate -- --all
```

That's the entry point for pre-commit, pre-merge, and pre-release. It runs
(in order): formatting, `nix flake check --no-build`, eval tests, host
builds, every `vm-*` VM scenario, `cargo test --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings`, and a nix build
of every Rust package (which runs cargo test under the nix sandbox).

For faster inner-loop iteration when `--all` is too much:

```sh
nix run .#validate                 # format + flake check + eval + hosts only
nix run .#validate -- --rust       # + cargo test + clippy + rust package builds
nix run .#validate -- --vm         # + every vm-* check
```

See `docs/mdbook/testing/overview.md` for what each tier contains and how
to drill down into a specific failure.

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
3. Ensure all checks pass: `nix run .#validate -- --all`
4. Open a PR with a clear description of the change and its motivation
5. One feature per PR — keep changes focused

## Architecture Decisions

Significant design decisions are recorded in `docs/adr/` as Architecture Decision Records (ADRs). Read the existing ADRs before proposing changes to core patterns like `mkHost`, hostSpec, or the agent state machine.

## License

By submitting a pull request, you agree to license your contribution under the project's applicable license:

- Contributions to `control-plane/` are licensed under **AGPL-3.0** ([LICENSE-AGPL](LICENSE-AGPL))
- Contributions to all other directories are licensed under **MIT** ([LICENSE-MIT](LICENSE-MIT))
