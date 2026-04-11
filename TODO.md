# TODO

Future work discovered during implementation. Grouped by source.

## Cycle state

The **nixfleet core hardening cycle** is closed as of Phase 4
(branch `hardening/core-checklist`). The cycle ran for four phases:

- **Phase 1 — Audit** (`docs/adr/009-core-hardening-audit.md`)
- **Phase 2 — Delete bloat** (PR #30, branch `hardening/core-delete`)
- **Phase 3 — Scenario tests** (PR #31, branch `hardening/core-scenarios`)
- **Phase 4 — Checklist coverage** (this branch)

Every Phase-4 plan item from the spec § 5 list and every previous-phase
follow-up tracked in this file is closed. The Phase 4 plan and its
locked decisions live in
`docs/superpowers/plans/2026-04-11-core-hardening-phase-4-checklist.md`
(gitignored — local working artifact).

The only remaining items are external dependencies that cannot be
fixed in this repository.

## Out-of-cycle (external)

- [ ] **#22: Revert `attic` input to upstream** when
  https://github.com/zhaofengli/attic/pull/300 is merged. External
  dependency — cannot be fixed in this repo until upstream lands.

- [ ] **Cosmetic:** Generation count fix in compliance probes (count
  the active system as generation 1, not 0). Lives in the
  `nixfleet-compliance` repository, not this one.
