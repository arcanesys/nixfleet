//! Post-switch verify: poll `/run/current-system` until expected, or terminal.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};

use super::types::{POLL_BUDGET, POLL_INTERVAL};

pub(super) async fn read_current_system_basename() -> Result<String> {
    let target = tokio::fs::read_link("/run/current-system")
        .await
        .with_context(|| "readlink /run/current-system")?;
    let basename = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| {
            anyhow!(
                "/run/current-system target has no utf-8 basename: {}",
                target.display()
            )
        })?
        .to_string();
    Ok(basename)
}

#[derive(Debug, Clone)]
pub enum PollOutcome {
    Settled,
    /// `last_observed` distinguishes "still running" from "switch died".
    Timeout { last_observed: String },
    /// Symlink at a third basename — caller must roll back.
    /// Only produced when caller set `previous_basename = Some(_)`.
    FlippedToUnexpected { observed: String },
}

/// `previous_basename = Some(p)` enables hard-mismatch detection: any third
/// basename → `FlippedToUnexpected` immediately. Rollback path leaves it None
/// (no meaningful pre-state). Read errors during polling are non-fatal.
pub struct VerifyPoll<'a> {
    pub expected_basename: &'a str,
    pub previous_basename: Option<&'a str>,
    pub interval: Duration,
    pub budget: Duration,
}

impl<'a> VerifyPoll<'a> {
    pub fn new(expected_basename: &'a str) -> Self {
        Self {
            expected_basename,
            previous_basename: None,
            interval: POLL_INTERVAL,
            budget: POLL_BUDGET,
        }
    }

    pub fn with_previous(mut self, previous: &'a str) -> Self {
        self.previous_basename = Some(previous);
        self
    }

    /// Pure: no logging, deterministic timing.
    pub async fn until_settled(&self) -> PollOutcome {
        let deadline = tokio::time::Instant::now() + self.budget;
        // unwrap_or_else fallback covers the budget=0 edge where no read runs.
        #[allow(unused_assignments)]
        let mut last_observed: Option<String> = None;

        loop {
            match read_current_system_basename().await {
                Ok(basename) => {
                    if basename == self.expected_basename {
                        return PollOutcome::Settled;
                    }
                    if let Some(prev) = self.previous_basename {
                        if basename != prev {
                            return PollOutcome::FlippedToUnexpected {
                                observed: basename,
                            };
                        }
                    }
                    last_observed = Some(basename);
                }
                Err(err) => {
                    // GOTCHA: symlink briefly absent mid-activation — read errors are non-fatal.
                    last_observed = Some(format!("<read-error: {err}>"));
                }
            }

            if tokio::time::Instant::now() >= deadline {
                return PollOutcome::Timeout {
                    last_observed: last_observed
                        .unwrap_or_else(|| String::from("<no-reads-completed>")),
                };
            }
            tokio::time::sleep(self.interval).await;
        }
    }
}
