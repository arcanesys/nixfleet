# GitHub Workflow

## Pull Requests

All changes go through PRs -- direct push to `main` is blocked.

### Branch Naming

Use `<type>/<description>`:
- `feat/new-scope`
- `fix/impermanence-path`
- `docs/architecture-update`

### CI Checks

Every PR runs:
1. **Format check** -- `nix fmt --fail-on-change`
2. **Validate** -- `nix flake check --no-build` (eval tests)

Both must pass before merge. PRs are squash-merged (one clean commit per feature).

## Issue Tracking

Track work items as GitHub Issues with labels:

- **Scope** (`scope:core`, `scope:scopes`, etc.) -- which part of the framework
- **Type** (`feature`, `bug`, `refactor`, `docs`, `infra`) -- what kind of work
- **Impact** (`impact:critical` to `impact:low`) -- priority

## Further Reading

- [Testing](testing.md) -- the test pyramid
- [Architecture](../../architecture.md) -- framework structure
