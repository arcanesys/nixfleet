//! NixFleet v0.2 control plane library.
//!
//! v0.2 is an Axum + SQLite + mTLS skeleton serving the four wire
//! endpoints from RFC-0003 §4. It reads trust from a JSON file
//! (`docs/trust-root-flow.md §3.2`), polls a local `fleet.resolved.json`
//! path, calls `nixfleet_reconciler::verify_artifact` on every load,
//! refuses unverified artifacts, and logs reconcile actions per tick.
//!
//! Every SQLite column carries a `-- derivable from: …` comment per
//! `docs/CONTRACTS.md §IV` (storage purity rule).
