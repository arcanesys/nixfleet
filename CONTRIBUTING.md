# Contributing to NixFleet

Thank you for your interest in contributing to NixFleet!

## Development Setup

**Prerequisites:**
- [Nix](https://nixos.org/download) with flakes enabled (`experimental-features = nix-command flakes`)

**Getting started:**
```sh
git clone https://github.com/arcanesys/nixfleet.git
cd nixfleet
nix develop  # enters the dev shell with Rust toolchain, cargo, rustfmt, clippy
```

## Project Structure

See [README.md](README.md#layout) for the directory layout.

## Running Tests

```sh
nix run .#validate -- --all        # full suite (format, eval, hosts, VM, Rust, clippy, package builds)
nix run .#validate                 # fast inner-loop (format + eval + hosts only)
nix run .#validate -- --rust       # + Rust tests and clippy
nix run .#validate -- --vm         # + VM tests
```

See [Testing Overview](docs/mdbook/testing/overview.md) for tier details and how to drill into specific failures.

## Code Style

- **Rust:** `rustfmt` defaults. Run `cargo fmt --all` before committing.
- **Nix:** `alejandra` formatter. Run `nix fmt` before committing.
- Pre-commit hooks enforce both automatically.

## Commit Conventions

Use [conventional commits](https://www.conventionalcommits.org/):

- `feat:` -- new feature
- `fix:` -- bug fix
- `docs:` -- documentation only
- `chore:` -- maintenance, dependencies, CI
- `test:` -- test additions or fixes
- `refactor:` -- code restructuring without behavior change

Keep the subject line under 72 characters. Use the body to explain "why," not "what."

## Pull Requests

1. Create a feature branch: `feat/`, `fix/`, `docs/`, etc.
2. Make your changes with tests
3. Ensure all checks pass: `nix run .#validate -- --all`
4. Open a PR with a clear description of the change and its motivation
5. One feature per PR -- keep changes focused

## Architecture Decisions

Significant design decisions are recorded in `docs/adr/` as Architecture Decision Records (ADRs). Read the existing ADRs before proposing changes to core patterns like `mkHost`, hostSpec, or the agent state machine.

## License

By submitting a pull request, you agree to license your contribution under the project's applicable license:

- Contributions to `control-plane/` are licensed under **AGPL-3.0** ([LICENSE-AGPL](LICENSE-AGPL))
- Contributions to all other directories are licensed under **MIT** ([LICENSE-MIT](LICENSE-MIT))
