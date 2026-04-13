//! `release create --push-hook` invokes the hook per store path.
//!
//! We cannot run `nix build` in the test, so we call
//! `cli::release::run_push_hook` directly with a hook that appends the
//! store path to a tempfile. This is the real function path the CLI
//! takes when `--push-hook` is set without `--push-to`.

use nixfleet::release::{extract_ssh_host, run_push_hook};
use std::fs;
use tempfile::tempdir;

#[test]
fn r3_push_hook_runs_locally_and_receives_store_path() {
    let dir = tempdir().unwrap();
    let trace = dir.path().join("hook-trace.txt");
    let trace_str = trace.to_str().unwrap();
    let hook = format!("echo {{}} >> {trace_str}");

    run_push_hook(None, &hook, "/nix/store/aaa-web-01", None, None, None).unwrap();
    run_push_hook(None, &hook, "/nix/store/bbb-web-02", None, None, None).unwrap();

    let contents = fs::read_to_string(&trace).unwrap();
    assert!(
        contents.contains("/nix/store/aaa-web-01"),
        "hook did not receive web-01 store path; trace = {contents:?}"
    );
    assert!(
        contents.contains("/nix/store/bbb-web-02"),
        "hook did not receive web-02 store path; trace = {contents:?}"
    );

    // Negative: a path that was never passed must NOT appear in the trace.
    assert!(
        !contents.contains("/nix/store/never-seen"),
        "hook trace contains a path that was never passed; trace = {contents:?}"
    );
}

#[test]
fn r3_push_hook_failing_command_surfaces_error() {
    // Hook exits non-zero → run_push_hook must return Err.
    let err = run_push_hook(None, "false", "/nix/store/ignored", None, None, None).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("push hook failed"),
        "expected 'push hook failed' in error; got: {msg}"
    );
}

#[test]
fn r3_extract_ssh_host_parses_url() {
    assert_eq!(
        extract_ssh_host("ssh://root@host"),
        Some("root@host".to_string())
    );
    assert_eq!(extract_ssh_host("ssh://host/"), Some("host".to_string()));

    // Negative: non-ssh scheme returns None.
    assert_eq!(extract_ssh_host("https://example.com"), None);
}
