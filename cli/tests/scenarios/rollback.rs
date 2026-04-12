//! `nixfleet rollback` tests.
//!
//! Since SSH is now the implicit (only) rollback mode, `--ssh` is
//! accepted but not required. The CLI attempts the SSH operation
//! directly.

use assert_cmd::Command;
use predicates::prelude::*;
use std::time::Duration;

#[test]
fn rb1_rollback_without_ssh_flag_attempts_operation() {
    // Without --ssh, rollback should still attempt the SSH operation
    // (and fail because the host is unreachable — NOT because --ssh
    // is missing).
    let mut cmd = Command::cargo_bin("nixfleet").expect("nixfleet binary");
    cmd.arg("rollback")
        .arg("--host")
        .arg("unreachable-host-99")
        .timeout(Duration::from_secs(10));

    cmd.assert()
        .failure()
        // Should NOT contain the old "requires --ssh" bail message
        .stderr(predicate::str::contains("requires --ssh").not());
}

#[test]
fn rb2_rollback_with_target_flag_accepted() {
    // --target flag should be accepted by the parser (no "unrecognized" error).
    // The command will fail (can't reach host) but the flag itself is valid.
    // Timeout prevents the test from hanging on unreachable SSH.
    let mut cmd = Command::cargo_bin("nixfleet").expect("nixfleet binary");
    cmd.args(["rollback", "--host", "web-01", "--target", "root@192.168.1.10"])
        .timeout(Duration::from_secs(10));

    cmd.assert()
        .failure()
        // Flag parsing succeeded — no "unrecognized" error
        .stderr(predicate::str::contains("unrecognized").not())
        // Should not say --ssh is required (implicit now)
        .stderr(predicate::str::contains("requires --ssh").not());
}
