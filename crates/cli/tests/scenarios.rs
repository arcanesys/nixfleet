// Single integration-test binary for the CLI crate.
// Each scenario module was formerly a separate binary; combining them
// into one eliminates redundant link passes.

#[path = "scenarios/config.rs"]
mod config;
#[path = "scenarios/harness.rs"]
mod harness;
#[path = "scenarios/release_delete.rs"]
mod release_delete;
#[path = "scenarios/release_hook.rs"]
mod release_hook;
#[path = "scenarios/rollback.rs"]
mod rollback;
#[path = "scenarios/subcommand_coverage.rs"]
mod subcommand_coverage;
