# Agent Self-Switch Resilience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the synchronous apply (spawn switch-to-configuration, wait for exit code) with a fire-and-forget pattern that survives the agent being killed by its own switch.

**Architecture:** The agent spawns `switch-to-configuration` in a detached transient systemd service (`systemd-run --unit=nixfleet-switch`), then polls `/run/current-system` until it matches the desired generation. If the agent gets killed mid-switch, it restarts and the same poll logic runs on startup. On poll timeout, the transient unit's exit status determines whether to retry or rollback.

**Tech Stack:** Rust (agent crate), systemd-run, tokio::time

---

## File Map

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `agent/src/nix.rs` | `fire_switch`, `poll_generation`, `check_switch_exit_status`, update `rollback`, remove `apply_generation`/`ApplyOutcome`/`is_lock_contention` |
| Modify | `agent/src/lib.rs` | Replace `apply_with_retry` with `fire_poll_with_retry`, update `run_deploy_cycle`, send "applying" report, remove old retry tests |

---

### Task 1: Add `poll_generation` to nix.rs

**Files:**
- Modify: `agent/src/nix.rs`

This is a pure async function that loops on `readlink`. Parameterized path
so tests can use a temp directory instead of `/run/current-system`.

- [ ] **Step 1: Write tests**

Add to the `#[cfg(test)] mod tests` block in `agent/src/nix.rs`:

```rust
#[tokio::test]
async fn test_poll_generation_matches_immediately() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("current-system");
    std::os::unix::fs::symlink("/nix/store/abc-target", &link).unwrap();

    let matched = poll_generation(
        "/nix/store/abc-target",
        &link,
        Duration::from_secs(10),
        Duration::from_millis(100),
    )
    .await
    .unwrap();
    assert!(matched);
}

#[tokio::test]
async fn test_poll_generation_times_out() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("current-system");
    std::os::unix::fs::symlink("/nix/store/abc-wrong", &link).unwrap();

    let matched = poll_generation(
        "/nix/store/abc-target",
        &link,
        Duration::from_secs(5),
        Duration::from_millis(100),
    )
    .await
    .unwrap();
    assert!(!matched);
}

#[tokio::test]
async fn test_poll_generation_detects_change() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("current-system");
    std::os::unix::fs::symlink("/nix/store/abc-old", &link).unwrap();

    let link_clone = link.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = std::fs::remove_file(&link_clone);
        std::os::unix::fs::symlink("/nix/store/abc-target", &link_clone).unwrap();
    });

    let matched = poll_generation(
        "/nix/store/abc-target",
        &link,
        Duration::from_secs(10),
        Duration::from_millis(500),
    )
    .await
    .unwrap();
    assert!(matched);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p nixfleet-agent -- test_poll_generation`
Expected: FAIL — `poll_generation` not found.

- [ ] **Step 3: Implement `poll_generation`**

Add above the `apply_generation` function in `agent/src/nix.rs`. Also add
`tempfile` to `[dev-dependencies]` in `agent/Cargo.toml` if not already present.

```rust
/// Poll a symlink path until it resolves to the expected store path.
///
/// Returns `Ok(true)` when the symlink target matches `expected`,
/// `Ok(false)` when `timeout` expires without a match. The `path`
/// parameter allows tests to use a temp directory instead of
/// `/run/current-system`.
pub async fn poll_generation(
    expected: &str,
    path: &std::path::Path,
    timeout: Duration,
    interval: Duration,
) -> Result<bool> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(target) = tokio::fs::read_link(path).await {
            if target.to_string_lossy() == expected {
                return Ok(true);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(interval).await;
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p nixfleet-agent -- test_poll_generation`
Expected: All 3 pass.

- [ ] **Step 5: Commit**

```bash
git add agent/src/nix.rs agent/Cargo.toml
git commit -m "feat(agent): add poll_generation for fire-and-forget apply

Polls a symlink path until it matches an expected store path or times
out. Parameterized path for testability. Uses tokio::time for paused-
time test support."
```

---

### Task 2: Add `fire_switch` to nix.rs

**Files:**
- Modify: `agent/src/nix.rs`

- [ ] **Step 1: Write test**

Add to the test module:

```rust
#[test]
fn test_fire_switch_command_construction() {
    let store_path = "/nix/store/abc123-nixos-system";
    let switch_bin = format!("{store_path}/bin/switch-to-configuration");
    let expected_args = [
        "systemd-run",
        "--unit=nixfleet-switch",
        "--",
        &switch_bin,
        "switch",
    ];
    assert_eq!(expected_args[0], "systemd-run");
    assert_eq!(expected_args[1], "--unit=nixfleet-switch");
    assert_eq!(expected_args[3], &switch_bin);
    assert_eq!(expected_args[4], "switch");
}
```

- [ ] **Step 2: Implement `fire_switch`**

Add above `poll_generation` in `agent/src/nix.rs`:

```rust
/// Timeout for the `systemd-run` spawn command itself (not the switch).
/// systemd-run queues the transient unit and returns almost instantly.
const FIRE_SWITCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Fire switch-to-configuration in a detached transient systemd service.
///
/// Spawns `systemd-run --unit=nixfleet-switch -- <switch-bin> switch`
/// and returns as soon as the transient unit is queued. The switch runs
/// asynchronously in `nixfleet-switch.service` — the agent does NOT
/// wait for it to complete. This allows the agent to survive being
/// killed by its own switch-to-configuration.
///
/// Errors on spawn failure or if the transient unit cannot be created
/// (e.g., a previous `nixfleet-switch.service` hasn't been cleaned up).
pub async fn fire_switch(store_path: &str) -> Result<()> {
    validate_store_path(store_path)?;
    let switch_bin = format!("{store_path}/bin/switch-to-configuration");
    info!(switch_bin, "Firing switch-to-configuration (detached)");

    let mut cmd = Command::new("systemd-run");
    cmd.args(["--unit=nixfleet-switch", "--", &switch_bin, "switch"]);
    let output = run_with_timeout(cmd, "systemd-run").await?;

    if !output.status.success() {
        let stderr = truncated_stderr(&output.stderr);
        anyhow::bail!("systemd-run failed to queue switch: {stderr}");
    }
    info!("Switch queued as nixfleet-switch.service");
    Ok(())
}
```

- [ ] **Step 3: Run all agent tests**

Run: `cargo test -p nixfleet-agent`
Expected: All tests pass (new test + existing).

- [ ] **Step 4: Commit**

```bash
git add agent/src/nix.rs
git commit -m "feat(agent): add fire_switch for detached switch-to-configuration

Spawns switch-to-configuration via systemd-run in a transient service
unit. Returns immediately — the switch runs asynchronously, surviving
agent restarts."
```

---

### Task 3: Add `check_switch_exit_status` to nix.rs

**Files:**
- Modify: `agent/src/nix.rs`

- [ ] **Step 1: Write tests**

```rust
#[test]
fn test_parse_switch_status_success() {
    let output = "ActiveState=inactive\nResult=success\n";
    assert_eq!(parse_switch_status(output), Some(true));
}

#[test]
fn test_parse_switch_status_failed() {
    let output = "ActiveState=inactive\nResult=exit-code\n";
    assert_eq!(parse_switch_status(output), Some(false));
}

#[test]
fn test_parse_switch_status_still_running() {
    let output = "ActiveState=active\nResult=success\n";
    assert_eq!(parse_switch_status(output), None);
}

#[test]
fn test_parse_switch_status_empty() {
    assert_eq!(parse_switch_status(""), None);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p nixfleet-agent -- test_parse_switch_status`
Expected: FAIL — `parse_switch_status` not found.

- [ ] **Step 3: Implement**

Add to `agent/src/nix.rs`:

```rust
/// Parse the output of `systemctl show nixfleet-switch.service -p ActiveState,Result`.
///
/// Returns `Some(true)` if the unit completed successfully,
/// `Some(false)` if it failed, or `None` if still running / not found.
fn parse_switch_status(output: &str) -> Option<bool> {
    let mut active_state = None;
    let mut result = None;
    for line in output.lines() {
        if let Some(val) = line.strip_prefix("ActiveState=") {
            active_state = Some(val);
        }
        if let Some(val) = line.strip_prefix("Result=") {
            result = Some(val);
        }
    }
    match (active_state, result) {
        (Some("inactive"), Some("success")) => Some(true),
        (Some("inactive"), Some(_)) => Some(false),
        _ => None,
    }
}

/// Check the exit status of the `nixfleet-switch.service` transient unit.
///
/// Returns `Some(true)` if it completed successfully, `Some(false)` if
/// it failed, or `None` if still running or not found.
pub async fn check_switch_exit_status() -> Result<Option<bool>> {
    let mut cmd = Command::new("systemctl");
    cmd.args(["show", "nixfleet-switch.service", "-p", "ActiveState,Result"]);
    let output = run_with_timeout(cmd, "systemctl show nixfleet-switch").await?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_switch_status(&stdout))
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p nixfleet-agent -- test_parse_switch_status`
Expected: All 4 pass.

- [ ] **Step 5: Commit**

```bash
git add agent/src/nix.rs
git commit -m "feat(agent): add check_switch_exit_status for transient unit outcome

Parses systemctl show output to determine if the detached switch
succeeded, failed, or is still running."
```

---

### Task 4: Replace apply mechanism in lib.rs

**Files:**
- Modify: `agent/src/lib.rs`

This is the core change. Replace `apply_with_retry` and the apply block in
`run_deploy_cycle` with the fire+poll+retry pattern.

- [ ] **Step 1: Write tests for the new retry logic**

Replace the entire `#[cfg(test)] mod tests` block in `agent/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fire_poll_retry_succeeds_immediately() {
        tokio::time::pause();
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("current-system");
        std::os::unix::fs::symlink("/nix/store/abc-target", &link).unwrap();

        let matched = nix::poll_generation(
            "/nix/store/abc-target",
            &link,
            Duration::from_secs(10),
            Duration::from_millis(100),
        )
        .await
        .unwrap();
        assert!(matched);
    }

    #[tokio::test]
    async fn test_poll_timeout_leads_to_retry_or_rollback() {
        tokio::time::pause();
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("current-system");
        std::os::unix::fs::symlink("/nix/store/abc-wrong", &link).unwrap();

        let matched = nix::poll_generation(
            "/nix/store/abc-target",
            &link,
            Duration::from_secs(5),
            Duration::from_millis(100),
        )
        .await
        .unwrap();
        assert!(!matched);
        // In production: check_switch_exit_status → retry or rollback
    }
}
```

- [ ] **Step 2: Replace constants and imports**

At the top of `agent/src/lib.rs`, replace:

```rust
use crate::nix::ApplyOutcome;

/// Maximum retries on activation lock contention.
const MAX_APPLY_RETRIES: u32 = 3;

/// Base delay for exponential backoff on lock contention.
const APPLY_RETRY_BASE: Duration = Duration::from_secs(5);
```

With:

```rust
/// Maximum retries when a fired switch fails (poll timeout + bad exit status).
const MAX_SWITCH_RETRIES: u32 = 3;

/// Timeout for polling `/run/current-system` after firing a switch.
const SWITCH_POLL_TIMEOUT: Duration = Duration::from_secs(300);

/// Interval between polls of `/run/current-system`.
const SWITCH_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Path to the current system symlink.
const CURRENT_SYSTEM_PATH: &str = "/run/current-system";
```

- [ ] **Step 3: Remove `apply_with_retry` and replace the apply block in `run_deploy_cycle`**

Replace lines 307-338 (the `// Apply: switch-to-configuration...` block through the
`apply_with_retry` match) with:

```rust
    // Apply: fire switch-to-configuration in a detached transient service,
    // then poll /run/current-system until it matches the desired generation.
    // The agent may be killed mid-switch (self-switch); on restart the
    // initial deploy cycle re-enters this path and the poll succeeds.
    metrics::record_state_transition("fetching", "applying");
    send_report(client, config, true, "applying").await;

    match nix::fire_switch(&desired.hash) {
        Ok(()) => {}
        Err(e) => {
            error!("Failed to fire switch: {e}");
            if let Err(se) = store.log_error(&format!("fire_switch failed: {e}")).await {
                warn!("store error: {se}");
            }
            metrics::record_state_transition("applying", "idle");
            return PollOutcome::Failed;
        }
    }

    let path = std::path::Path::new(CURRENT_SYSTEM_PATH);
    let applied = match nix::poll_generation(
        &desired.hash,
        path,
        SWITCH_POLL_TIMEOUT,
        SWITCH_POLL_INTERVAL,
    )
    .await
    {
        Ok(true) => true,
        Ok(false) => {
            // Poll timed out. Check switch exit status and retry if failed.
            warn!("Switch poll timed out, checking transient unit status");
            let mut applied = false;
            for attempt in 1..=MAX_SWITCH_RETRIES {
                match nix::check_switch_exit_status().await {
                    Ok(Some(false)) => {
                        warn!(attempt, max = MAX_SWITCH_RETRIES, "Switch failed, retrying");
                        if let Err(e) = nix::fire_switch(&desired.hash) {
                            error!("Retry fire_switch failed: {e}");
                            break;
                        }
                        match nix::poll_generation(
                            &desired.hash,
                            path,
                            SWITCH_POLL_TIMEOUT,
                            SWITCH_POLL_INTERVAL,
                        )
                        .await
                        {
                            Ok(true) => {
                                applied = true;
                                break;
                            }
                            Ok(false) => continue,
                            Err(e) => {
                                error!("Poll error on retry: {e}");
                                break;
                            }
                        }
                    }
                    _ => {
                        warn!("Switch status inconclusive, giving up");
                        break;
                    }
                }
            }
            applied
        }
        Err(e) => {
            error!("Poll error: {e}");
            false
        }
    };

    if applied {
        info!(hash = %desired.hash, "Generation applied");
    } else {
        error!("Failed to apply generation after retries");
        metrics::record_state_transition("applying", "rolling_back");
        rollback_and_report(client, config, store, "switch timed out after retries").await;
        return PollOutcome::Success { poll_hint };
    }
```

- [ ] **Step 5: Verify compilation**

Run: `cargo test -p nixfleet-agent`
Expected: All tests pass (compile check — the new tests don't exercise the full deploy cycle).

- [ ] **Step 6: Commit**

```bash
git add agent/src/lib.rs
git commit -m "feat(agent): replace synchronous apply with fire-and-forget pattern

Fire switch-to-configuration via systemd-run, poll /run/current-system
until generation matches. Send 'applying' report before firing. Retry
the fire+poll cycle on switch failure. The agent survives being killed
by its own switch — on restart, the existing startup deploy cycle
re-enters the poll path."
```

---

### Task 5: Update `rollback` to use fire+poll

**Files:**
- Modify: `agent/src/nix.rs:144-196`

- [ ] **Step 1: Update `rollback()` to use `fire_switch` + `poll_generation`**

Replace the end of `rollback()` (from the profile symlink resolution onward):

```rust
    // Resolve profile symlink to store path
    let store_path = tokio::fs::read_link(&prev_path)
        .await
        .context("failed to resolve profile symlink to store path")?;
    let store_path_str = store_path.to_string_lossy();

    // Fire the rollback switch in a detached transient service
    fire_switch(&store_path_str)?;

    // Poll until the system switches to the previous generation
    let path = std::path::Path::new("/run/current-system");
    let timeout = Duration::from_secs(300);
    let interval = Duration::from_secs(2);
    if poll_generation(&store_path_str, path, timeout, interval).await? {
        Ok(())
    } else {
        anyhow::bail!("rollback timed out: /run/current-system did not match {store_path_str}")
    }
```

- [ ] **Step 2: Verify compilation and tests**

Run: `cargo test -p nixfleet-agent`
Expected: All tests pass.

- [ ] **Step 3: Commit**

```bash
git add agent/src/nix.rs
git commit -m "feat(agent): rollback uses fire-and-forget pattern

Rollback now fires switch-to-configuration via systemd-run and polls
for the previous generation, same as the apply path."
```

---

### Task 6: Remove obsoleted code

**Files:**
- Modify: `agent/src/nix.rs`
- Modify: `agent/src/lib.rs`

- [ ] **Step 1: Remove `ApplyOutcome`, `is_lock_contention`, `apply_generation` from nix.rs**

Delete:
- `ApplyOutcome` enum (lines 99-108)
- `is_lock_contention` function (lines 110-118)
- `apply_generation` function (lines 120-142)
- Tests: `test_is_lock_contention_matches_common_patterns`, `test_is_lock_contention_rejects_unrelated_errors`, `test_switch_bin_path_construction`

Keep all other tests (validate_store_path, truncated_stderr, generation parsing, path construction, poll_generation, fire_switch, parse_switch_status).

- [ ] **Step 2: Clean up lib.rs imports and remove dead code**

In `agent/src/lib.rs`:
- Remove `use crate::nix::ApplyOutcome;`
- Remove `apply_with_retry` function (the old generic retry wrapper) if still present

- [ ] **Step 3: Verify compilation and all tests pass**

Run: `cargo test -p nixfleet-agent`
Expected: All tests pass. Removed tests no longer counted.

- [ ] **Step 4: Commit**

```bash
git add agent/src/nix.rs agent/src/lib.rs
git commit -m "refactor(agent): remove synchronous apply_generation and lock detection

ApplyOutcome, is_lock_contention, and apply_generation are replaced by
the fire_switch + poll_generation + check_switch_exit_status pipeline.
Lock contention is detected indirectly via poll timeout + exit status."
```

---

### Task 7: Update TODO.md and run full validation

**Files:**
- Modify: `TODO.md`

- [ ] **Step 1: Mark self-switch item as done in TODO.md**

Change:

```markdown
- [ ] **Agent: survive self-switch (fire-and-forget apply).**
```

To:

```markdown
- [x] **Agent: survive self-switch (fire-and-forget apply).**
```

- [ ] **Step 2: Commit**

```bash
git add TODO.md
git commit -m "docs: mark agent self-switch resilience as done"
```

- [ ] **Step 3: Run cargo fmt**

Run: `cargo fmt`

If anything changed:
```bash
git add -A && git commit -m "style: cargo fmt"
```

- [ ] **Step 4: Run full validation**

Run: `cargo test -p nixfleet-agent`
Expected: All tests pass.

Run: `cargo clippy -p nixfleet-agent -- -D warnings`
Expected: No warnings.
