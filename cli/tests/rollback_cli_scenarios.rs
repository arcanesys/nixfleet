//! `nixfleet rollback` tests.
//!
//! Since SSH is now the implicit (only) rollback mode, `--ssh` is
//! accepted but not required. The CLI attempts the SSH operation
//! directly.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn rb1_rollback_without_ssh_flag_attempts_operation() {
    // Without --ssh, rollback should still attempt the SSH operation
    // (and fail because the host is unreachable — NOT because --ssh
    // is missing).
    let mut cmd = Command::cargo_bin("nixfleet").expect("nixfleet binary");
    cmd.arg("rollback").arg("--host").arg("unreachable-host-99");

    cmd.assert()
        .failure()
        // Should NOT contain the old "requires --ssh" bail message
        .stderr(predicate::str::contains("requires --ssh").not())
        // Should attempt SSH (and fail because host is unreachable)
        .stderr(predicate::str::contains("SSH").or(predicate::str::contains("ssh")));
}

#[test]
fn rb2_rollback_with_target_parses_correctly() {
    // --target flag should be accepted alongside --host
    let mut cmd = Command::cargo_bin("nixfleet").expect("nixfleet binary");
    cmd.args(["rollback", "--host", "web-01", "--target", "root@192.168.1.10"]);

    // Will fail (can't reach host) but should not fail on flag parsing
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized").not());
}
