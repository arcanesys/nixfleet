# TODO

Future work that cannot be closed inside this repository.

## Internal

- [ ] **Release GC.** Releases accumulate in the CP database.
  Health report GC exists (`cleanup_old_health_reports`), but old
  releases are never pruned. See ADR-008.
