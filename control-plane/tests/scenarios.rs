// Single integration-test binary for the control-plane crate.
// Each scenario module was formerly a separate binary; combining them
// into one eliminates redundant link passes.

#[path = "scenarios/harness.rs"]
mod harness;
#[path = "scenarios/audit.rs"]
mod audit;
#[path = "scenarios/auth.rs"]
mod auth;
#[path = "scenarios/cn_validation.rs"]
mod cn_validation;
#[path = "scenarios/deploy.rs"]
mod deploy;
#[path = "scenarios/failure.rs"]
mod failure;
#[path = "scenarios/hydration.rs"]
mod hydration;
#[path = "scenarios/machine.rs"]
mod machine;
#[path = "scenarios/metrics.rs"]
mod metrics;
#[path = "scenarios/migrations.rs"]
mod migrations;
#[path = "scenarios/polling.rs"]
mod polling;
#[path = "scenarios/release.rs"]
mod release;
#[path = "scenarios/rollback.rs"]
mod rollback;
#[path = "scenarios/route_coverage.rs"]
mod route_coverage;
