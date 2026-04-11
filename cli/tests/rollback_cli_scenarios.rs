//! RB3 — `nixfleet rollback` without `--ssh` bails with actionable guidance.
//!
//! Spec: docs/superpowers/specs/2026-04-10-core-hardening-cycle-design.md Section 4
//! Audit: docs/adr/009-core-hardening-audit.md Category 1 (CLI subcommands)
//!
//! The control-plane set-generation rollback endpoint was removed. The CLI
//! must refuse non-SSH rollbacks and point the user at the supported paths
//! (release create + deploy, or `--on-failure revert`). This test asserts
//! the exact guidance text from `cli/src/main.rs::rollback`.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn rb3_rollback_without_ssh_bails_with_guidance() {
    let mut cmd = Command::cargo_bin("nixfleet").expect("nixfleet binary");
    cmd.arg("rollback").arg("--host").arg("web-01");

    cmd.assert()
        .failure()
        // Primary refusal phrase — unique to this error path.
        .stderr(predicate::str::contains(
            "nixfleet rollback requires --ssh mode",
        ))
        // Guidance clause pointing at the supported forward-rollback path.
        .stderr(predicate::str::contains("release create"))
        // Guidance clause pointing at the on-failure revert path.
        .stderr(predicate::str::contains("--on-failure revert"))
        // Negative assertion: the CLI must NOT have started an actual
        // rollback. `println!("Rolling back ...")` is only reached after
        // the --ssh check passes, so its absence proves we bailed early.
        .stderr(predicate::str::contains("Rolling back").not())
        .stdout(predicate::str::contains("Rolling back").not());
}
